//! Lightweight boundary-span facade shared by native SCXML crates and Wasm adapters.
//!
//! Native code starts spans through the global OpenTelemetry API provider. The
//! crate installs no SDK, exporter, sampler, runtime, or network transport. Wasm
//! components use [`WasmSpanAdapter`] to map the same bounded operation metadata
//! to imported WIT tracing capabilities. That WIT contract is intentionally a
//! project facade, not a claim of finalized WASI `OTel` compatibility.
//!
//! # Example
//!
//! Telemetry lives at boundaries only: a component or WIT entry point, a
//! public facade operation, a capability or I/O call, or a host lifecycle
//! step opens one span with a closed module and operation label and marks
//! failures with a closed category. Private helpers, getters, conversions,
//! and hot inner loops open no span of their own; they run inside the
//! enclosing boundary span, and their failures surface as its typed failure.
//! Tests capture the same observations through a thread-scoped in-memory
//! [`Recorder`]:
//!
//! ```
//! use scxml_telemetry::{
//!     ErrorCategory, Module, Operation, Recorder, SignalEvent, install_recorder, observe,
//! };
//!
//! /// Public boundary: decodes one untrusted Machine IR version field.
//! pub fn decode_ir_version(bytes: &str) -> Result<u32, std::num::ParseIntError> {
//!     let span = observe(Module::CompilerCore, Operation::CompilerDecodeIr);
//!     let result = parse_version(bytes);
//!     if result.is_ok() {
//!         span.event(SignalEvent::InputAccepted);
//!     } else {
//!         span.event(SignalEvent::InputRejected);
//!         span.fail(ErrorCategory::InvalidInput);
//!     }
//!     result
//! }
//!
//! /// Private helper: no span of its own; the boundary span covers it.
//! fn parse_version(bytes: &str) -> Result<u32, std::num::ParseIntError> {
//!     bytes.trim().parse::<u32>()
//! }
//!
//! let recorder = Recorder::default();
//! {
//!     let _guard = install_recorder(recorder.clone());
//!     assert!(decode_ir_version("not a number").is_err());
//! }
//! let observations = recorder.observations();
//! assert_eq!(observations.len(), 1, "only the boundary is observed");
//! let failed = &observations[0];
//! assert_eq!(failed.operation, Operation::CompilerDecodeIr);
//! assert_eq!(failed.error, Some(ErrorCategory::InvalidInput));
//! assert_eq!(failed.events, [SignalEvent::InputRejected]);
//! ```

#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::str::FromStr as _;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use opentelemetry::Context;
use opentelemetry::trace::{
    SpanContext, SpanId, TraceContextExt as _, TraceFlags, TraceId, TraceState,
};
#[cfg(not(target_arch = "wasm32"))]
use opentelemetry::trace::{Status, Tracer as _};

/// Stable, code-defined module identity used as a bounded telemetry dimension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Module {
    /// Conformance manifest loading and validation.
    ConformanceManifest,
    /// Conformance report and comparison logic.
    ConformanceReport,
    /// Conformance orchestration.
    ConformanceRunner,
    /// Native official-corpus preparation.
    ConformancePrepare,
    /// Command-line composition root.
    Cli,
    /// Canonical artifact encoding, decoding, and rendering.
    Reporting,
    /// Compiler-core artifact production.
    CompilerCore,
    /// Validator Wasm component adapter.
    ValidatorComponent,
    /// Compiler Wasm component adapter.
    CompilerComponent,
    /// Rustc reporter Wasm component adapter.
    ReporterRustcComponent,
    /// SARIF reporter Wasm component adapter.
    ReporterSarifComponent,
    /// Wasm machine component adapter.
    MachineComponent,
    /// Deterministic clock component and domain logic.
    ClockComponent,
    /// SCXML I/O processor component adapter.
    IoProcessorComponent,
    /// `BasicHTTP` Event I/O processor component and domain logic.
    IoProcessorBasicHttpComponent,
    /// ECMAScript data-model component and domain logic.
    DataModelEcmascriptComponent,
    /// Null data-model component and domain logic.
    DataModelNullComponent,
    /// `XPath` data-model component and domain logic.
    DataModelXpathComponent,
    /// Source-preserving XML parsing and the syntax tree.
    Syntax,
    /// Native validation pipeline (parse, schema, and semantic rules).
    Validator,
    /// Example extension component.
    ExtensionExampleComponent,
    /// Optional native Wasmtime host adapter.
    HostWasmtime,
    /// Deterministic SCXML execution kernel.
    Engine,
    /// Ahead-of-time machine component packager.
    Packager,
    /// External `<script src>` resolution and Wasm script-component checks.
    Script,
    /// Build-time fuel metering of core Wasm modules for fuel-less hosts.
    WasmMeter,
}

impl Module {
    /// Returns the bounded wire representation used by native and WIT adapters.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConformanceManifest => "conformance-manifest",
            Self::ConformanceReport => "conformance-report",
            Self::ConformanceRunner => "conformance-runner",
            Self::ConformancePrepare => "conformance-prepare",
            Self::Cli => "cli",
            Self::Reporting => "reporting",
            Self::CompilerCore => "compiler-core",
            Self::ValidatorComponent => "validator-component",
            Self::CompilerComponent => "compiler-component",
            Self::ReporterRustcComponent => "reporter-rustc-component",
            Self::ReporterSarifComponent => "reporter-sarif-component",
            Self::MachineComponent => "machine-component",
            Self::ClockComponent => "clock-component",
            Self::IoProcessorComponent => "io-processor-component",
            Self::IoProcessorBasicHttpComponent => "io-processor-basichttp-component",
            Self::DataModelEcmascriptComponent => "data-model-ecmascript-component",
            Self::DataModelNullComponent => "data-model-null-component",
            Self::DataModelXpathComponent => "data-model-xpath-component",
            Self::Syntax => "syntax",
            Self::Validator => "validator",
            Self::ExtensionExampleComponent => "extension-example-component",
            Self::HostWasmtime => "host-wasmtime",
            Self::Engine => "engine",
            Self::Packager => "packager",
            Self::Script => "script",
            Self::WasmMeter => "wasm-meter",
        }
    }
}

/// Stable operation identifiers. Values are release-bounded source constants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    /// Loads a suite manifest and its files.
    ManifestLoadSuite,
    /// Writes prepared corpus metadata.
    ManifestWriteSuite,
    /// Hashes an artifact file.
    ManifestSha256File,
    /// Constructs a deterministic engine.
    EngineNew,
    /// Starts an engine.
    EngineStart,
    /// Dispatches one event to an engine.
    EngineSend,
    /// Resumes pending internal engine work.
    EngineResume,
    /// Cancels a running engine session by exiting its whole configuration.
    EngineCancel,
    /// Builds the engine's read-only session image (no state change).
    EngineSnapshot,
    /// Queues one typed platform error event.
    EngineQueuePlatformError,
    /// Aborts one executable-content block with a typed platform error.
    EngineBlockFailed,
    /// Runs one host command to a stable configuration through the engine driver.
    EngineRun,
    /// Writes report JSON.
    ReportWriteJson,
    /// Reads report JSON.
    ReportReadJson,
    /// Runs a prepared corpus path.
    RunnerRunCorpus,
    /// Runs an already-loaded suite.
    RunnerRunSuite,
    /// Runs one selected case.
    RunnerRunCase,
    /// Reads transformer identity and version.
    PrepareTransformerIdentity,
    /// Executes one XSLT transform.
    PrepareTransformerTransform,
    /// Prepares one official corpus.
    PrepareCorpus,
    /// Acquires the official manifest explicitly.
    PrepareAcquireManifest,
    /// Loads and validates one checked-in IRP errata file.
    PrepareLoadErrata,
    /// Acquires one corpus file explicitly.
    PrepareAcquireCorpusFile,
    /// Downloads one explicitly requested corpus file.
    PrepareDownload,
    /// CLI process entry point.
    CliMain,
    /// Runs validation.
    CliValidate,
    /// Reads and bounds one SCXML source file.
    CliReadSource,
    /// Reads and bounds one machine template file.
    CliReadTemplate,
    /// Reads and bounds one external script package file.
    CliReadScript,
    /// Reads the schema files a document's `event:schema` attributes
    /// reference, relative to the document.
    CliReadEventSchema,
    /// Loads one document a run names by URL (`<invoke src>`,
    /// `<script src>`, `<data src>`, `srcexpr`): a `file:` read or a bounded
    /// `https:` fetch, once per root session.
    CliReadDocument,
    /// Reads and verifies the CLI's extension-package manifests and schemas.
    CliReadExtensionPackage,
    /// Reads and verifies the CLI's `--service-package` manifests.
    CliReadServicePackage,
    /// Resolves one repository namespace's package (manifest and schema) from
    /// the cache or its repository.
    CliResolveExtensionPackage,
    /// Resolves one repository namespace's extension component from the cache
    /// or its release, before a run binds it.
    CliResolveExtensionComponent,
    /// Resolves one repository service's package (manifest, optional
    /// schema, and event schemas) from the cache or its repository.
    CliResolveServicePackage,
    /// Resolves one repository service's component from the cache or its
    /// release, before a run binds it.
    CliResolveServiceComponent,
    /// Runs compilation.
    CliCompile,
    /// Runs conformance.
    CliConformance,
    /// Proves snapshot collection at every checkpoint of a conformance run
    /// (`conformance run --snapshot-every-checkpoint`).
    CliConformanceSnapshotProof,
    /// Runs official corpus preparation.
    CliConformancePrepare,
    /// Emits CLI diagnostics.
    CliEmit,
    /// Renders the long-form explanation of one diagnostic code.
    CliExplain,
    /// Validates an artifact envelope.
    ReportingArtifactValidate,
    /// Encodes canonical artifact bytes.
    ReportingEncodeCanonical,
    /// Decodes canonical artifact bytes.
    ReportingDecodeCanonical,
    /// Renders rustc-style diagnostics.
    ReportingRustcRender,
    /// Renders SARIF diagnostics.
    ReportingRenderSarif,
    /// Runs the validator component export.
    ValidatorComponentValidate,
    /// Parses and validates one SCXML source through the native validator.
    ValidatorValidateSource,
    /// Validates one already parsed SCXML document.
    ValidatorValidateDocument,
    /// Reads one registered schema file from the filesystem.
    ValidatorSchemaFileRead,
    /// Runs XSD validation for one document against its resolved schemas.
    ValidatorXsdValidate,
    /// Runs the compiler component export.
    CompilerComponentCompile,
    /// Runs the rustc reporter component export.
    ReporterRustcRender,
    /// Runs the SARIF reporter component export.
    ReporterSarifRender,
    /// Schedules one caller-identified timer.
    ClockSchedule,
    /// Cancels one caller-identified timer.
    ClockCancel,
    /// Drains a bounded due-timer batch.
    ClockDrainDue,
    /// Advances logical time.
    ClockAdvance,
    /// Validates a portable SCXML send request.
    IoProcessorPrepare,
    /// Validates, stamps, and schedules or routes one send (WIT
    /// `io-processor-api.dispatch`).
    IoProcessorDispatch,
    /// Decodes the pending delayed sends from the clock's read-only image
    /// (WIT `io-processor-api.snapshot`).
    IoProcessorSnapshot,
    /// Performs one `BasicHTTP` send through the transport capability.
    BasicHttpSend,
    /// Validates and decodes one inbound `BasicHTTP` message.
    BasicHttpReceive,
    /// Initializes an ECMAScript data-model instance.
    DataModelInitialize,
    /// Evaluates an ECMAScript condition.
    DataModelEvaluateCondition,
    /// Evaluates an ECMAScript value.
    DataModelEvaluateValue,
    /// Evaluates an ECMAScript `<log>` expression to its deterministic text.
    DataModelEvaluateLog,
    /// Assigns an ECMAScript location.
    DataModelAssign,
    /// Executes bounded ECMAScript source.
    DataModelExecute,
    /// Dumps the ECMAScript system values and variables read-only.
    DataModelSnapshot,
    /// Parse-only syntax check of one expression, location, or script.
    DataModelCheckSyntax,
    /// Parse-only side-effect analysis of one document's conditions and
    /// `<log expr>` values.
    DataModelCheckEffects,
    /// Initializes an `XPath` data-model instance.
    XpathDataModelInitialize,
    /// Evaluates an `XPath` condition.
    XpathDataModelEvaluateCondition,
    /// Evaluates an `XPath` value, string, or `<foreach>` node-set.
    XpathDataModelEvaluateValue,
    /// Assigns an `XPath` location or binds a variable.
    XpathDataModelAssign,
    /// Executes an `XPath` `<script>` (the `XPath` data model has no scripting).
    XpathDataModelExecute,
    /// Dumps the `XPath` system values and variables read-only.
    XpathDataModelSnapshot,
    /// Parse-only `XPath` syntax check.
    XpathDataModelCheckSyntax,
    /// Evaluates a null data-model `In()` condition.
    NullDataModelEvaluateCondition,
    /// Gathers and encodes one session snapshot at a Wasmtime checkpoint.
    HostWasmtimeSessionSnapshot,
    /// Runs a document and writes checkpoint snapshots (`scxml snapshot run`).
    CliSnapshotRun,
    /// Runs one long-lived live session over NDJSON (`scxml run`).
    CliRun,
    /// Renders one snapshot document (`scxml snapshot show`).
    CliSnapshotShow,
    /// Checks one snapshot document (`scxml snapshot check`).
    CliSnapshotCheck,
    /// Diffs two snapshot documents (`scxml snapshot diff`).
    CliSnapshotDiff,
    /// Executes one example executable-content extension.
    ExtensionExampleExecute,
    /// Constructs a Wasmtime component engine.
    HostWasmtimeComponentEngine,
    /// Starts one authenticated machine component and runs its lifecycle.
    HostWasmtimeComponentExecute,
    /// Runs one Wasmtime machine session over an external event sequence.
    HostWasmtimeMachineSession,
    /// Links and executes the bounded capability-component graph.
    HostWasmtimeCapabilityGraphExecute,
    /// Authenticates one component artifact against its pinned identity.
    HostWasmtimeAuthenticateArtifact,
    /// Constructs a Wasmtime store with fuel and resource limits.
    HostWasmtimeStore,
    /// Starts a host-owned guest span.
    HostWasmtimeTelemetryStart,
    /// Parses and bounds one untrusted XML document.
    SyntaxParseXml,
    /// Builds the canonical compiler report artifact.
    CompilerCanonicalArtifact,
    /// Validates, pins, and lowers one source into a package recipe.
    CompilerCompile,
    /// Lowers one namespace-resolved SCXML document to Machine IR.
    CompilerLower,
    /// Validates every Machine IR invariant.
    CompilerValidateIr,
    /// Encodes validated Machine IR as canonical JSON.
    CompilerEncodeIr,
    /// Decodes untrusted Machine IR JSON.
    CompilerDecodeIr,
    /// Packages lowered Machine IR and a pinned template into a component.
    PackagerPackage,
    /// Validates a caller-supplied machine template identity and world.
    PackagerValidateTemplate,
    /// Serves host-supplied Machine IR through the `ir-source` import.
    HostWasmtimeIrSource,
    /// Authenticates one resolved external script (bounds, digest, kind).
    ScriptVerify,
    /// Validates one Wasm script component against the `script` world.
    ScriptValidateComponent,
    /// Resolves and prepares one external script at document load.
    HostWasmtimeScriptLoad,
    /// Runs one loaded external script against the active data model.
    HostWasmtimeScriptRun,
    /// Instantiates, identity-checks, and initializes one session's
    /// extension component.
    HostWasmtimeExtensionInstantiate,
    /// Loads the extension components a session's document uses at session
    /// start (`machine-capabilities.load-extensions`).
    HostWasmtimeExtensionLoad,
    /// Executes one executable-content extension through its extension
    /// component.
    HostWasmtimeExtensionExecute,
    /// Shuts down one session's extension component.
    HostWasmtimeExtensionShutdown,
    /// Starts one invocation of an invoked service: instantiates its
    /// component and calls its `invoked-service-api.invoke`
    /// (`machine-capabilities.start-invocation`).
    HostWasmtimeServiceStart,
    /// Delivers one event to an invoked service's `events` stream.
    HostWasmtimeServiceSend,
    /// Fires an invoked service's due timers
    /// (`wasi:clocks/monotonic-clock.wait-until`).
    HostWasmtimeServiceWaitUntil,
    /// Cancels one invoked service (SCXML 6.4): cancels its `invoke`
    /// subtask and drops its store.
    HostWasmtimeServiceCancel,
    /// One outbound HTTP request an invoked service made through the
    /// host-granted `outbound-http.fetch`, from admission to its response.
    HostWasmtimeServiceHttpFetch,
    /// Performs one invoked service's admitted request through the
    /// embedder's transport.
    HostWasmtimeServiceHttpRequest,
    /// Instantiates and links one data-model component.
    HostWasmtimeDataModelInstantiate,
    /// Instantiates and links one `BasicHTTP` I/O processor component.
    HostWasmtimeBasicHttpInstantiate,
    /// Performs one outbound `BasicHTTP` send through the component.
    HostWasmtimeBasicHttpSend,
    /// Decodes one inbound `BasicHTTP` message through the component.
    HostWasmtimeBasicHttpReceive,
    /// Performs one `BasicHTTP` component transport POST through the embedder.
    HostWasmtimeBasicHttpTransportPost,
    /// Compiles and links the components of one Wasmtime session host.
    HostWasmtimeSessionHostNew,
    /// Runs one root session (and its children) to completion on the
    /// Wasmtime session host.
    HostWasmtimeSessionRun,
    /// Opens one live root session: constructs, starts, and runs it to a stable configuration.
    HostWasmtimeSessionOpen,
    /// Processes one external event of a live session to a stable configuration.
    HostWasmtimeSessionSend,
    /// Fires the due delayed sends of a live session.
    HostWasmtimeSessionAdvance,
    /// Closes one live session: exits the root and drops every session.
    HostWasmtimeSessionClose,
    /// Delivers one inbound `BasicHTTP` message to a live session (SCXML
    /// C.2) and processes it to a stable configuration.
    HostWasmtimeSessionReceiveHttp,
    /// Delivers the `error.communication` events owed for `BasicHTTP` sends
    /// that failed after they were accepted, and processes them to
    /// a stable configuration.
    HostWasmtimeSessionCommunicationError,
    /// Delivers the HTTP response of one invoked service's request to its
    /// invocation and processes it to a stable configuration.
    HostWasmtimeSessionHttpResponse,
    /// Performs one outbound `BasicHTTP` POST over the network, off the
    /// session thread.
    HostWasmtimeBasicHttpNetworkPost,
    /// Reads and answers one request of the built-in `BasicHTTP` listener.
    HostWasmtimeBasicHttpListenerRequest,
    /// One failed `accept` of the built-in `BasicHTTP` listener.
    HostWasmtimeBasicHttpListenerAccept,
    /// One connection the built-in `BasicHTTP` listener accepted but could
    /// not serve or keep as configured: dropped unanswered, or served on a
    /// thread the listener could not keep.
    HostWasmtimeBasicHttpListenerConnection,
    /// Constructs, links, and starts one invoked session.
    HostWasmtimeInvokedSessionStart,
    /// Cancels and drops one invoked session.
    HostWasmtimeInvokedSessionCancel,
    /// Compiles one invoked child document through the compiler component.
    HostWasmtimeInvokeCompile,
    /// Performs one in-process `BasicHTTP` loopback POST between sessions.
    HostWasmtimeLoopbackPost,
    /// Validates one document with the validator component, its expression
    /// syntax linked to one data-model component.
    HostWasmtimeValidatorValidate,
    /// Compiles and links the validator component with its expression-syntax
    /// data-model component.
    HostWasmtimeValidatorNew,
    /// Runs one package validator component call (one package's targets in
    /// one document).
    HostWasmtimePackageValidatorValidate,
    /// Runs one conformance case through the shipped components under
    /// Wasmtime.
    ConformanceRunnerRunCaseWasm,
    /// Instruments one core module with a self-metering fuel counter.
    WasmMeterInstrument,
    /// Runs the `scxml-wasm-meter` command line.
    WasmMeterCli,
}

impl Operation {
    /// Returns the static span name for this operation.
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive match is the single bounded source of every operation name"
    )]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ManifestLoadSuite => "scxml.conformance.manifest.load-suite",
            Self::ManifestWriteSuite => "scxml.conformance.manifest.write-suite",
            Self::ManifestSha256File => "scxml.conformance.manifest.sha256-file",
            Self::EngineNew => "scxml.engine.new",
            Self::EngineStart => "scxml.engine.start",
            Self::EngineSend => "scxml.engine.send",
            Self::EngineResume => "scxml.engine.resume",
            Self::EngineCancel => "scxml.engine.cancel",
            Self::EngineSnapshot => "scxml.engine.snapshot",
            Self::EngineQueuePlatformError => "scxml.engine.queue-platform-error",
            Self::EngineBlockFailed => "scxml.engine.block-failed",
            Self::EngineRun => "scxml.engine.run",
            Self::ReportWriteJson => "scxml.conformance.report.write-json",
            Self::ReportReadJson => "scxml.conformance.report.read-json",
            Self::RunnerRunCorpus => "scxml.conformance.runner.run-corpus",
            Self::RunnerRunSuite => "scxml.conformance.runner.run-suite",
            Self::RunnerRunCase => "scxml.conformance.runner.run-case",
            Self::ConformanceRunnerRunCaseWasm => "scxml.conformance.runner.run-case-wasm",
            Self::HostWasmtimeSessionHostNew => "scxml.host-wasmtime.session-host-new",
            Self::HostWasmtimeSessionRun => "scxml.host-wasmtime.session-run",
            Self::HostWasmtimeSessionOpen => "scxml.host-wasmtime.session-open",
            Self::HostWasmtimeSessionSend => "scxml.host-wasmtime.session-send",
            Self::HostWasmtimeSessionAdvance => "scxml.host-wasmtime.session-advance",
            Self::HostWasmtimeSessionClose => "scxml.host-wasmtime.session-close",
            Self::HostWasmtimeSessionReceiveHttp => "scxml.host-wasmtime.session-receive-http",
            Self::HostWasmtimeSessionCommunicationError => {
                "scxml.host-wasmtime.session-communication-error"
            }
            Self::HostWasmtimeSessionHttpResponse => "scxml.host-wasmtime.session-http-response",
            Self::HostWasmtimeBasicHttpNetworkPost => "scxml.host-wasmtime.basichttp-network-post",
            Self::HostWasmtimeBasicHttpListenerRequest => {
                "scxml.host-wasmtime.basichttp-listener-request"
            }
            Self::HostWasmtimeBasicHttpListenerAccept => {
                "scxml.host-wasmtime.basichttp-listener-accept"
            }
            Self::HostWasmtimeBasicHttpListenerConnection => {
                "scxml.host-wasmtime.basichttp-listener-connection"
            }
            Self::HostWasmtimeInvokedSessionStart => "scxml.host-wasmtime.invoked-session-start",
            Self::HostWasmtimeInvokedSessionCancel => "scxml.host-wasmtime.invoked-session-cancel",
            Self::HostWasmtimeInvokeCompile => "scxml.host-wasmtime.invoke-compile",
            Self::HostWasmtimeLoopbackPost => "scxml.host-wasmtime.loopback-post",
            Self::HostWasmtimeValidatorValidate => "scxml.host-wasmtime.validator-validate",
            Self::HostWasmtimeValidatorNew => "scxml.host-wasmtime.validator-new",
            Self::HostWasmtimePackageValidatorValidate => {
                "scxml.host-wasmtime.package-validator-validate"
            }
            Self::PrepareTransformerIdentity => "scxml.conformance.prepare.transformer-identity",
            Self::PrepareTransformerTransform => "scxml.conformance.prepare.transformer-transform",
            Self::PrepareCorpus => "scxml.conformance.prepare.corpus",
            Self::PrepareAcquireManifest => "scxml.conformance.prepare.acquire-manifest",
            Self::PrepareLoadErrata => "scxml.conformance.prepare.load-errata",
            Self::PrepareAcquireCorpusFile => "scxml.conformance.prepare.acquire-corpus-file",
            Self::PrepareDownload => "scxml.conformance.prepare.download",
            Self::CliMain => "scxml.cli.main",
            Self::CliValidate => "scxml.cli.validate",
            Self::CliReadSource => "scxml.cli.read-source",
            Self::CliReadTemplate => "scxml.cli.read-template",
            Self::CliReadScript => "scxml.cli.read-script",
            Self::CliReadEventSchema => "scxml.cli.read-event-schema",
            Self::CliReadDocument => "scxml.cli.read-document",
            Self::CliReadExtensionPackage => "scxml.cli.read-extension-package",
            Self::CliReadServicePackage => "scxml.cli.read-service-package",
            Self::CliResolveExtensionPackage => "scxml.cli.resolve-extension-package",
            Self::CliResolveExtensionComponent => "scxml.cli.resolve-extension-component",
            Self::CliResolveServicePackage => "scxml.cli.resolve-service-package",
            Self::CliResolveServiceComponent => "scxml.cli.resolve-service-component",
            Self::CliCompile => "scxml.cli.compile",
            Self::CliConformance => "scxml.cli.conformance",
            Self::CliConformanceSnapshotProof => "scxml.cli.conformance-snapshot-proof",
            Self::CliConformancePrepare => "scxml.cli.conformance-prepare",
            Self::CliEmit => "scxml.cli.emit",
            Self::CliExplain => "scxml.cli.explain",
            Self::ReportingArtifactValidate => "scxml.reporting.artifact-validate",
            Self::ReportingEncodeCanonical => "scxml.reporting.encode-canonical",
            Self::ReportingDecodeCanonical => "scxml.reporting.decode-canonical",
            Self::ReportingRustcRender => "scxml.reporting.rustc-render",
            Self::ReportingRenderSarif => "scxml.reporting.render-sarif",
            Self::ValidatorComponentValidate => "scxml.validator-component.validate",
            Self::ValidatorValidateSource => "scxml.validator.validate-source",
            Self::ValidatorValidateDocument => "scxml.validator.validate-document",
            Self::ValidatorSchemaFileRead => "scxml.validator.schema-file-read",
            Self::ValidatorXsdValidate => "scxml.validator.xsd-validate",
            Self::CompilerComponentCompile => "scxml.compiler-component.compile",
            Self::ReporterRustcRender => "scxml.reporter-rustc-component.render",
            Self::ReporterSarifRender => "scxml.reporter-sarif-component.render",
            Self::ClockSchedule => "scxml.clock.schedule",
            Self::ClockCancel => "scxml.clock.cancel",
            Self::ClockDrainDue => "scxml.clock.drain-due",
            Self::ClockAdvance => "scxml.clock.advance",
            Self::IoProcessorPrepare => "scxml.io-processor.prepare",
            Self::IoProcessorDispatch => "scxml.io-processor.dispatch",
            Self::IoProcessorSnapshot => "scxml.io-processor.snapshot",
            Self::BasicHttpSend => "scxml.io-processor-basichttp.send",
            Self::BasicHttpReceive => "scxml.io-processor-basichttp.receive",
            Self::DataModelInitialize => "scxml.data-model-ecmascript.initialize",
            Self::DataModelEvaluateCondition => "scxml.data-model-ecmascript.evaluate-condition",
            Self::DataModelEvaluateValue => "scxml.data-model-ecmascript.evaluate-value",
            Self::DataModelEvaluateLog => "scxml.data-model-ecmascript.evaluate-log",
            Self::DataModelAssign => "scxml.data-model-ecmascript.assign",
            Self::DataModelExecute => "scxml.data-model-ecmascript.execute",
            Self::DataModelSnapshot => "scxml.data-model-ecmascript.snapshot",
            Self::DataModelCheckSyntax => "scxml.data-model-ecmascript.check-syntax",
            Self::DataModelCheckEffects => "scxml.data-model-ecmascript.check-effects",
            Self::XpathDataModelInitialize => "scxml.data-model-xpath.initialize",
            Self::XpathDataModelEvaluateCondition => "scxml.data-model-xpath.evaluate-condition",
            Self::XpathDataModelEvaluateValue => "scxml.data-model-xpath.evaluate-value",
            Self::XpathDataModelAssign => "scxml.data-model-xpath.assign",
            Self::XpathDataModelExecute => "scxml.data-model-xpath.execute",
            Self::XpathDataModelSnapshot => "scxml.data-model-xpath.snapshot",
            Self::XpathDataModelCheckSyntax => "scxml.data-model-xpath.check-syntax",
            Self::NullDataModelEvaluateCondition => "scxml.data-model-null.evaluate-condition",
            Self::HostWasmtimeSessionSnapshot => "scxml.host-wasmtime.session-snapshot",
            Self::CliSnapshotRun => "scxml.cli.snapshot-run",
            Self::CliRun => "scxml.cli.run",
            Self::CliSnapshotShow => "scxml.cli.snapshot-show",
            Self::CliSnapshotCheck => "scxml.cli.snapshot-check",
            Self::CliSnapshotDiff => "scxml.cli.snapshot-diff",
            Self::ExtensionExampleExecute => "scxml.extension-example.execute",
            Self::HostWasmtimeComponentEngine => "scxml.host-wasmtime.component-engine",
            Self::HostWasmtimeComponentExecute => "scxml.host-wasmtime.component-execute",
            Self::HostWasmtimeMachineSession => "scxml.host-wasmtime.machine-session",
            Self::HostWasmtimeCapabilityGraphExecute => {
                "scxml.host-wasmtime.capability-graph-execute"
            }
            Self::HostWasmtimeAuthenticateArtifact => "scxml.host-wasmtime.authenticate-artifact",
            Self::HostWasmtimeStore => "scxml.host-wasmtime.store",
            Self::HostWasmtimeTelemetryStart => "scxml.host-wasmtime.telemetry-start",
            Self::SyntaxParseXml => "scxml.syntax.parse-xml",
            Self::CompilerCanonicalArtifact => "scxml.compiler.canonical-artifact",
            Self::CompilerCompile => "scxml.compiler.compile",
            Self::CompilerLower => "scxml.compiler.lower",
            Self::CompilerValidateIr => "scxml.compiler.validate-ir",
            Self::CompilerEncodeIr => "scxml.compiler.encode-ir",
            Self::CompilerDecodeIr => "scxml.compiler.decode-ir",
            Self::PackagerPackage => "scxml.packager.package",
            Self::PackagerValidateTemplate => "scxml.packager.validate-template",
            Self::HostWasmtimeIrSource => "scxml.host-wasmtime.ir-source",
            Self::ScriptVerify => "scxml.script.verify",
            Self::ScriptValidateComponent => "scxml.script.validate-component",
            Self::HostWasmtimeScriptLoad => "scxml.host-wasmtime.script-load",
            Self::HostWasmtimeScriptRun => "scxml.host-wasmtime.script-run",
            Self::HostWasmtimeExtensionInstantiate => "scxml.host-wasmtime.extension-instantiate",
            Self::HostWasmtimeExtensionLoad => "scxml.host-wasmtime.extension-load",
            Self::HostWasmtimeExtensionExecute => "scxml.host-wasmtime.extension-execute",
            Self::HostWasmtimeExtensionShutdown => "scxml.host-wasmtime.extension-shutdown",
            Self::HostWasmtimeServiceStart => "scxml.host-wasmtime.service-start",
            Self::HostWasmtimeServiceSend => "scxml.host-wasmtime.service-send",
            Self::HostWasmtimeServiceWaitUntil => "scxml.host-wasmtime.service-wait-until",
            Self::HostWasmtimeServiceCancel => "scxml.host-wasmtime.service-cancel",
            Self::HostWasmtimeServiceHttpFetch => "scxml.host-wasmtime.service-http-fetch",
            Self::HostWasmtimeServiceHttpRequest => "scxml.host-wasmtime.service-http-request",
            Self::HostWasmtimeDataModelInstantiate => "scxml.host-wasmtime.data-model-instantiate",
            Self::HostWasmtimeBasicHttpInstantiate => "scxml.host-wasmtime.basichttp-instantiate",
            Self::HostWasmtimeBasicHttpSend => "scxml.host-wasmtime.basichttp-send",
            Self::HostWasmtimeBasicHttpReceive => "scxml.host-wasmtime.basichttp-receive",
            Self::HostWasmtimeBasicHttpTransportPost => {
                "scxml.host-wasmtime.basichttp-transport-post"
            }
            Self::WasmMeterInstrument => "scxml.wasm-meter.instrument",
            Self::WasmMeterCli => "scxml.wasm-meter.cli",
        }
    }
}

/// Bounded error classifications safe for telemetry status recording.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCategory {
    /// Invalid or rejected caller input.
    InvalidInput,
    /// Manifest or external data could not be read.
    InputRead,
    /// Parsing or decoding failed.
    Parse,
    /// Validation rejected input.
    Validation,
    /// Lowering or model adaptation failed.
    Lowering,
    /// Capability is not supported.
    Unsupported,
    /// Engine execution failed.
    Execution,
    /// Oracle comparison failed.
    Oracle,
    /// Report serialization or storage failed.
    Report,
    /// Harness invariant failed.
    Harness,
    /// Host/component capability failed.
    Platform,
    /// A bounded work budget was exceeded.
    Budget,
    /// Component packaging or composition failed.
    Packaging,
}

/// Bounded OTEL events emitted by component and artifact boundaries.
///
/// Event names are source constants. Source names, event names, diagnostic
/// messages, and other user-controlled values must not be attached here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SignalEvent {
    /// Validated external input was accepted at a trust boundary.
    InputAccepted,
    /// External input was rejected at a trust boundary without committing state.
    InputRejected,
    /// An explicit external acquisition completed successfully.
    ExternalReadCompleted,
    /// A deterministic transformation completed successfully.
    TransformationCompleted,
    /// Canonical artifact bytes passed fail-closed decoding.
    ArtifactAccepted,
    /// An authoritative producer emitted canonical artifact bytes.
    ArtifactProduced,
    /// A digest-bound immutable artifact was reused under the same identity.
    ArtifactReused,
    /// A reporter finished rendering an accepted artifact.
    RenderCompleted,
    /// An SCXML event entered a machine boundary.
    ScxmlEventReceived,
    /// An SCXML event left a machine boundary.
    ScxmlEventEmitted,
    /// A deterministic timer was scheduled.
    ClockScheduled,
    /// A deterministic timer was cancelled.
    ClockCancelled,
    /// A bounded due-timer batch was drained.
    ClockDueDrained,
    /// Logical time advanced.
    ClockAdvanced,
    /// A clock operation rejected input without committing state.
    ClockRejected,
    /// The SCXML I/O processor returned its static type URI.
    IoProcessorTypeUriReturned,
    /// The SCXML I/O processor accepted a send request.
    IoProcessorSendAccepted,
    /// The SCXML I/O processor rejected a send request.
    IoProcessorSendRejected,
    /// The SCXML I/O processor completed a cancellation request.
    IoProcessorCancelCompleted,
    /// The SCXML I/O processor rejected a cancellation request.
    IoProcessorCancelRejected,
    /// The SCXML I/O processor mapped or rendered a bounded failure.
    IoProcessorFailureReported,
    /// The optional in-memory recorder exhausted its observation identifiers.
    RecorderIdExhausted,
}

impl SignalEvent {
    /// Returns the bounded OTEL event name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputAccepted => "scxml.input.accepted",
            Self::InputRejected => "scxml.input.rejected",
            Self::ExternalReadCompleted => "scxml.external-read.completed",
            Self::TransformationCompleted => "scxml.transformation.completed",
            Self::ArtifactAccepted => "scxml.artifact.accepted",
            Self::ArtifactProduced => "scxml.artifact.produced",
            Self::ArtifactReused => "scxml.artifact.reused",
            Self::RenderCompleted => "scxml.report.rendered",
            Self::ScxmlEventReceived => "scxml.event.received",
            Self::ScxmlEventEmitted => "scxml.event.emitted",
            Self::ClockScheduled => "scxml.clock.scheduled",
            Self::ClockCancelled => "scxml.clock.cancelled",
            Self::ClockDueDrained => "scxml.clock.due-drained",
            Self::ClockAdvanced => "scxml.clock.advanced",
            Self::ClockRejected => "scxml.clock.rejected",
            Self::IoProcessorTypeUriReturned => "scxml.io-processor.type-uri-returned",
            Self::IoProcessorSendAccepted => "scxml.io-processor.send-accepted",
            Self::IoProcessorSendRejected => "scxml.io-processor.send-rejected",
            Self::IoProcessorCancelCompleted => "scxml.io-processor.cancel-completed",
            Self::IoProcessorCancelRejected => "scxml.io-processor.cancel-rejected",
            Self::IoProcessorFailureReported => "scxml.io-processor.failure-reported",
            Self::RecorderIdExhausted => "scxml.telemetry.recorder-id-exhausted",
        }
    }
}

impl ErrorCategory {
    /// Returns the bounded wire representation for status/error recording.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid-input",
            Self::InputRead => "input-read",
            Self::Parse => "parse",
            Self::Validation => "validation",
            Self::Lowering => "lowering",
            Self::Unsupported => "unsupported",
            Self::Execution => "execution",
            Self::Oracle => "oracle",
            Self::Report => "report",
            Self::Harness => "harness",
            Self::Platform => "platform",
            Self::Budget => "budget",
            Self::Packaging => "packaging",
        }
    }
}

/// Portable parent context passed across the WIT tracing boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceContext {
    /// W3C `traceparent` value, when supplied by a host adapter.
    pub traceparent: Option<String>,
    /// W3C `tracestate` value, when supplied by a host adapter.
    pub tracestate: Option<String>,
}

/// Maximum accepted bytes in a W3C `traceparent` field.
pub const MAX_TRACEPARENT_BYTES: usize = 55;

/// Maximum accepted bytes in a combined W3C `tracestate` field.
pub const MAX_TRACESTATE_BYTES: usize = 512;

/// Safe validation failures for an untrusted W3C trace carrier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceContextError {
    /// `tracestate` was supplied without its required `traceparent`.
    TraceparentMissing,
    /// A carrier field exceeded its explicit byte bound.
    FieldTooLong,
    /// The version, separators, hexadecimal fields, IDs, or flags were invalid.
    TraceparentInvalid,
    /// The bounded `tracestate` list was invalid.
    TracestateInvalid,
}

impl fmt::Display for TraceContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::TraceparentMissing => "tracestate requires traceparent",
            Self::FieldTooLong => "trace context field exceeds its byte bound",
            Self::TraceparentInvalid => "traceparent is not canonical W3C version 00",
            Self::TracestateInvalid => "tracestate is not a valid W3C list",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for TraceContextError {}

/// Bounded numeric measurements emitted by runtime adapters.
///
/// Each enum value selects one fixed instrument and unit. Samples have no
/// metric attributes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeMeasurement {
    /// Initial Wasmtime fuel assigned to a store.
    WasmtimeFuel,
    /// Maximum bytes allowed for each Wasmtime linear memory.
    WasmtimeMemoryBytes,
    /// Maximum elements allowed for each Wasmtime table.
    WasmtimeTableElements,
    /// Maximum Wasmtime instances allowed in one store.
    WasmtimeInstances,
    /// Maximum Wasmtime tables allowed in one store.
    WasmtimeTables,
    /// Maximum Wasmtime memories allowed in one store.
    WasmtimeMemories,
    /// Maximum Wasm stack bytes configured on the engine.
    WasmtimeStackBytes,
}

/// Bounded numeric measurements emitted by canonical artifact boundaries.
///
/// Each enum value selects one fixed instrument and unit. Samples have no
/// metric attributes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactMeasurement {
    /// Encoded prepared-suite manifest bytes read at the boundary.
    SuiteManifestBytes,
    /// Prepared cases verified at the boundary.
    SuiteCases,
    /// Encoded conformance-report bytes read at the boundary.
    ReportBytes,
    /// Conformance case results verified at the boundary.
    ReportCases,
}

impl ArtifactMeasurement {
    const fn instrument(self) -> (&'static str, &'static str) {
        match self {
            Self::SuiteManifestBytes => ("scxml.artifact.suite-manifest.size", "By"),
            Self::SuiteCases => ("scxml.artifact.suite.cases", "{case}"),
            Self::ReportBytes => ("scxml.artifact.report.size", "By"),
            Self::ReportCases => ("scxml.artifact.report.cases", "{case}"),
        }
    }
}

impl RuntimeMeasurement {
    const fn instrument(self) -> (&'static str, &'static str) {
        match self {
            Self::WasmtimeFuel => ("scxml.runtime.fuel", "{fuel}"),
            Self::WasmtimeMemoryBytes => ("scxml.runtime.memory.limit", "By"),
            Self::WasmtimeTableElements => ("scxml.runtime.table-elements.limit", "{element}"),
            Self::WasmtimeInstances => ("scxml.runtime.instances.limit", "{instance}"),
            Self::WasmtimeTables => ("scxml.runtime.tables.limit", "{table}"),
            Self::WasmtimeMemories => ("scxml.runtime.memories.limit", "{memory}"),
            Self::WasmtimeStackBytes => ("scxml.runtime.stack.limit", "By"),
        }
    }
}

/// Validates and extracts a bounded W3C carrier into an OpenTelemetry context.
///
/// Absence of both fields is accepted as an empty context. Invalid input has
/// no side effects and the returned error never contains carrier bytes.
///
/// # Errors
///
/// Returns [`TraceContextError`] when field presence, byte bounds, canonical
/// version-00 syntax, IDs, flags, or trace-state members are invalid.
pub fn extract_trace_context(carrier: &TraceContext) -> Result<Context, TraceContextError> {
    let Some(traceparent) = carrier.traceparent.as_deref() else {
        return if carrier.tracestate.is_some() {
            Err(TraceContextError::TraceparentMissing)
        } else {
            Ok(Context::new())
        };
    };
    if traceparent.len() > MAX_TRACEPARENT_BYTES
        || carrier
            .tracestate
            .as_ref()
            .is_some_and(|value| value.len() > MAX_TRACESTATE_BYTES)
    {
        return Err(TraceContextError::FieldTooLong);
    }
    let mut parts = traceparent.split('-');
    let version = parts.next();
    let trace_id_text = parts.next();
    let span_id_text = parts.next();
    let flags_text = parts.next();
    if version != Some("00") || parts.next().is_some() {
        return Err(TraceContextError::TraceparentInvalid);
    }
    let (Some(trace_id_text), Some(span_id_text), Some(flags_text)) =
        (trace_id_text, span_id_text, flags_text)
    else {
        return Err(TraceContextError::TraceparentInvalid);
    };
    if trace_id_text.len() != 32
        || span_id_text.len() != 16
        || flags_text.len() != 2
        || !traceparent
            .bytes()
            .all(|byte| byte == b'-' || byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TraceContextError::TraceparentInvalid);
    }
    let trace_id =
        TraceId::from_hex(trace_id_text).map_err(|_| TraceContextError::TraceparentInvalid)?;
    let span_id =
        SpanId::from_hex(span_id_text).map_err(|_| TraceContextError::TraceparentInvalid)?;
    let flags =
        u8::from_str_radix(flags_text, 16).map_err(|_| TraceContextError::TraceparentInvalid)?;
    if trace_id == TraceId::INVALID || span_id == SpanId::INVALID || flags > 1 {
        return Err(TraceContextError::TraceparentInvalid);
    }
    let trace_state = match carrier.tracestate.as_deref() {
        Some(value) => {
            TraceState::from_str(value).map_err(|_| TraceContextError::TracestateInvalid)?
        }
        None => TraceState::NONE,
    };
    let span_context =
        SpanContext::new(trace_id, span_id, TraceFlags::new(flags), true, trace_state);
    Ok(Context::new().with_remote_span_context(span_context))
}

/// Encodes the current span from an OpenTelemetry context as a W3C carrier.
///
/// An invalid or absent span produces an empty carrier. No SDK or exporter is
/// initialized by this operation.
#[must_use]
pub fn encode_trace_context(context: &Context) -> TraceContext {
    let span = context.span();
    let span_context = span.span_context();
    if !span_context.is_valid() {
        return TraceContext {
            traceparent: None,
            tracestate: None,
        };
    }
    let traceparent = format!(
        "00-{}-{}-{:02x}",
        span_context.trace_id(),
        span_context.span_id(),
        span_context.trace_flags().to_u8()
    );
    let tracestate = span_context.trace_state().header();
    TraceContext {
        traceparent: Some(traceparent),
        tracestate: (!tracestate.is_empty()).then_some(tracestate),
    }
}

/// Records a numeric runtime limit or budget through the global OTEL meter.
///
/// `measurement` selects a fixed instrument and unit; `value` is recorded as
/// one attribute-free sample. The operation does not install a provider or
/// exporter.
pub fn record_runtime_measurement(measurement: RuntimeMeasurement, value: u64) {
    let (instrument, unit) = measurement.instrument();
    let meter = opentelemetry::global::meter("scxml");
    meter
        .u64_histogram(instrument)
        .with_unit(unit)
        .build()
        .record(value, &[]);
}

/// Records a numeric canonical-artifact size through the global OTEL meter.
///
/// `measurement` selects a fixed instrument and unit; `value` is recorded as
/// one attribute-free sample. The operation does not install a provider or
/// exporter.
pub fn record_artifact_measurement(measurement: ArtifactMeasurement, value: u64) {
    let (instrument, unit) = measurement.instrument();
    opentelemetry::global::meter("scxml")
        .u64_histogram(instrument)
        .with_unit(unit)
        .build()
        .record(value, &[]);
}

/// One closed function observation captured by the test recorder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    /// Recorder-local observation identifier.
    pub id: u64,
    /// Parent recorder-local identifier.
    pub parent_id: Option<u64>,
    /// Validated remote or ambient OTEL parent, excluding baggage.
    pub parent_trace_context: Option<TraceContext>,
    /// Bounded module identifier.
    pub module: Module,
    /// Bounded operation identifier.
    pub operation: Operation,
    /// Whether the function ended with an explicitly marked failure.
    pub error: Option<ErrorCategory>,
    /// Bounded OTEL events recorded on this function span.
    pub events: Vec<SignalEvent>,
}

/// Scoped in-memory recording sink used by deterministic tests.
///
/// Observation identifiers are unique nonzero `u64` values. After the finite
/// identifier space is exhausted, the recorder stops capturing new
/// observations and reports one bounded [`SignalEvent::RecorderIdExhausted`];
/// production OpenTelemetry spans continue normally.
#[derive(Clone, Default)]
pub struct Recorder {
    inner: Arc<RecorderInner>,
}

#[derive(Default)]
struct RecorderInner {
    /// Last identifier issued from this recorder; zero is reserved as unissued.
    #[cfg(not(target_arch = "wasm32"))]
    next_id: AtomicU64,
    #[cfg(not(target_arch = "wasm32"))]
    id_exhaustion_reported: AtomicBool,
    observations: std::sync::Mutex<Vec<Observation>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecorderIdAllocation {
    Allocated(u64),
    Exhausted,
}

/// Restores the recorder previously installed on this thread when dropped.
pub struct RecorderGuard {
    previous: Option<Recorder>,
    not_send: PhantomData<Rc<()>>,
}

/// Installs a recorder on the current thread until the returned guard is dropped.
///
/// Observations started on other threads are never captured by this recorder,
/// so independent tests may install recorders concurrently. The guard cannot
/// cross threads and nested installations restore in last-in, first-out order.
/// Production OpenTelemetry remains process-global; this optional in-memory
/// sink does not install or replace an OpenTelemetry provider.
#[must_use]
pub fn install_recorder(recorder: Recorder) -> RecorderGuard {
    let previous = RECORDER_SLOT.with_borrow_mut(|slot| slot.replace(recorder));
    RecorderGuard {
        previous,
        not_send: PhantomData,
    }
}

impl Recorder {
    /// Returns a stable snapshot of closed observations in close order.
    #[must_use]
    pub fn observations(&self) -> Vec<Observation> {
        self.inner
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn allocate_id(&self) -> RecorderIdAllocation {
        let previous = self
            .inner
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .ok();
        match previous.and_then(|value| value.checked_add(1)) {
            Some(id) => RecorderIdAllocation::Allocated(id),
            None => RecorderIdAllocation::Exhausted,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn take_id_exhaustion_signal(&self) -> bool {
        !self
            .inner
            .id_exhaustion_reported
            .swap(true, Ordering::AcqRel)
    }
}

impl Drop for RecorderGuard {
    fn drop(&mut self) {
        RECORDER_SLOT.with_borrow_mut(|slot| *slot = self.previous.take());
    }
}

/// RAII function observation. Dropping it closes the span on every return path.
///
/// # Mutation and sharing
///
/// A span is owned by the one boundary call that created it and is never
/// shared. [`Self::event`] and [`Self::fail`] take `&self` and mutate the
/// span's own state through a `RefCell` (the recorded events, one entry per
/// call) and a `Cell` (the error category; the last call wins). Neither holds
/// a borrow across any other call, so they never panic. Only [`Drop`] reads
/// that state, to end the OpenTelemetry span with its status, record the
/// call metric, and hand the observation to an installed [`Recorder`].
///
/// Natively, creation attaches the span's context to the calling thread and
/// sets that thread's current recorder observation; dropping restores both.
/// The type is therefore `!Send` and `!Sync`: drop it on the thread that
/// created it, and drop nested spans in reverse creation order (as scoped
/// RAII does). In a Wasm guest the span is a local marker with the same
/// `&self` mutation and no thread or host state.
///
/// ```compile_fail
/// fn assert_send<T: Send>() {}
/// assert_send::<scxml_telemetry::FunctionSpan>();
/// ```
///
/// ```compile_fail
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<scxml_telemetry::FunctionSpan>();
/// ```
#[cfg(not(target_arch = "wasm32"))]
pub struct FunctionSpan {
    context: Option<opentelemetry::ContextGuard>,
    span_context: Context,
    recorder: Option<RecorderState>,
    error: Cell<Option<ErrorCategory>>,
    events: RefCell<Vec<SignalEvent>>,
    module: Module,
    operation: Operation,
}

/// Zero-cost guest marker; component code reports through its versioned WIT facade.
///
/// [`Self::event`] and [`Self::fail`] mutate its own `RefCell` and `Cell`
/// through `&self`, as on native targets; the type is `!Sync` and is owned by
/// the one call that created it.
#[cfg(target_arch = "wasm32")]
pub struct FunctionSpan {
    error: Cell<Option<ErrorCategory>>,
    events: RefCell<Vec<SignalEvent>>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct RecorderState {
    recorder: Recorder,
    id: u64,
    parent_id: Option<u64>,
    parent_trace_context: Option<TraceContext>,
    module: Module,
    operation: Operation,
}

/// Starts and enters a native OpenTelemetry span for one boundary operation.
#[must_use]
pub fn observe(module: Module, operation: Operation) -> FunctionSpan {
    start_observation(module, operation, &Context::current())
}

#[cfg(target_arch = "wasm32")]
fn start_observation(_module: Module, _operation: Operation, _parent: &Context) -> FunctionSpan {
    FunctionSpan {
        error: Cell::new(None),
        events: RefCell::new(Vec::new()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn start_observation(module: Module, operation: Operation, parent: &Context) -> FunctionSpan {
    let tracer = opentelemetry::global::tracer("scxml");
    let span = tracer.start_with_context(operation.as_str(), parent);
    let span_context = parent.with_span(span);
    span_context.span().set_attributes([
        opentelemetry::KeyValue::new("scxml.module", module.as_str()),
        opentelemetry::KeyValue::new("scxml.operation", operation.as_str()),
    ]);
    let context = span_context.clone().attach();
    let recorder = active_recorder().and_then(|recorder| match recorder.allocate_id() {
        RecorderIdAllocation::Allocated(id) => {
            let parent_id = CURRENT_RECORDING.with(Cell::get);
            let parent_trace_context =
                Some(encode_trace_context(parent)).filter(|carrier| carrier.traceparent.is_some());
            CURRENT_RECORDING.with(|current| current.set(Some(id)));
            Some(RecorderState {
                recorder,
                id,
                parent_id,
                parent_trace_context,
                module,
                operation,
            })
        }
        RecorderIdAllocation::Exhausted => {
            report_recorder_id_exhaustion(&span_context, &recorder);
            None
        }
    });
    FunctionSpan {
        context: Some(context),
        span_context,
        recorder,
        error: Cell::new(None),
        events: RefCell::new(Vec::new()),
        module,
        operation,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn report_recorder_id_exhaustion(context: &Context, recorder: &Recorder) {
    if recorder.take_id_exhaustion_signal() {
        let event = SignalEvent::RecorderIdExhausted;
        context.span().add_event(event.as_str(), Vec::new());
        record_event_metric(event);
    }
}

impl FunctionSpan {
    /// Adds one bounded event to the current OTEL span.
    pub fn event(&self, event: SignalEvent) {
        #[cfg(not(target_arch = "wasm32"))]
        self.span_context
            .span()
            .add_event(event.as_str(), Vec::new());
        self.events.borrow_mut().push(event);
        #[cfg(not(target_arch = "wasm32"))]
        record_event_metric(event);
    }

    /// Marks the observation as failed using only a bounded error category.
    pub fn fail(&self, category: ErrorCategory) {
        self.error.set(Some(category));
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.span_context
                .span()
                .set_status(Status::error(category.as_str()));
            self.span_context
                .span()
                .set_attribute(opentelemetry::KeyValue::new(
                    "error.type",
                    category.as_str(),
                ));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for FunctionSpan {
    fn drop(&mut self) {
        self.context.take();
        self.span_context.span().end();
        record_span_metric(self.module, self.operation, self.error.get());
        if let Some(state) = self.recorder.take() {
            CURRENT_RECORDING.with(|current| current.set(state.parent_id));
            state
                .recorder
                .inner
                .observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Observation {
                    id: state.id,
                    parent_id: state.parent_id,
                    parent_trace_context: state.parent_trace_context,
                    module: state.module,
                    operation: state.operation,
                    error: self.error.get(),
                    events: self.events.get_mut().clone(),
                });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn record_span_metric(module: Module, operation: Operation, error: Option<ErrorCategory>) {
    let counter = opentelemetry::global::meter("scxml")
        .u64_counter("scxml.function.calls")
        .build();
    let module = opentelemetry::KeyValue::new("scxml.module", module.as_str());
    let operation = opentelemetry::KeyValue::new("scxml.operation", operation.as_str());
    match error {
        Some(category) => counter.add(
            1,
            &[
                module,
                operation,
                opentelemetry::KeyValue::new("scxml.outcome", "failed"),
                opentelemetry::KeyValue::new("error.type", category.as_str()),
            ],
        ),
        None => counter.add(
            1,
            &[
                module,
                operation,
                opentelemetry::KeyValue::new("scxml.outcome", "completed"),
            ],
        ),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn record_event_metric(event: SignalEvent) {
    opentelemetry::global::meter("scxml")
        .u64_counter("scxml.events")
        .build()
        .add(
            1,
            &[opentelemetry::KeyValue::new(
                "scxml.event.name",
                event.as_str(),
            )],
        );
}

/// Imported-WIT bridge implemented by generated guest bindings.
pub trait WasmTraceHost {
    /// Host-owned span resource or handle.
    type Span;

    /// Starts a span with bounded code metadata and optional propagated context.
    fn start(
        &self,
        module: Module,
        operation: Operation,
        parent: Option<&TraceContext>,
    ) -> (Self::Span, Option<TraceContext>);

    /// Ends a span and records a bounded error category when present.
    fn end(&self, span: Self::Span, error: Option<ErrorCategory>);

    /// Adds one bounded event to a host-owned span.
    fn event(&self, span: &Self::Span, event: SignalEvent);
}

/// RAII adapter for a Wasm component's imported WIT tracing host.
///
/// # Mutation and sharing
///
/// The adapter owns one host span from [`Self::new`] until it is dropped.
/// [`Self::fail`] records the error category in a `Cell` through `&self` (the
/// last call wins). [`Self::event`] borrows the span `RefCell` immutably for
/// the one host call, so an event forwarded by `H::event` that re-enters
/// [`Self::event`] or [`Self::fail`] on the same adapter does not panic. Only
/// [`Drop`] takes the span out of the `RefCell`, ending it exactly once
/// through `H::end` with the recorded category. The type is `!Sync` because
/// of the cells: it belongs to the component call that created it, borrows
/// `host` for its whole life, and is `Send` only when `H` is `Sync` and
/// `H::Span` is `Send`.
pub struct WasmSpanAdapter<'a, H: WasmTraceHost> {
    host: &'a H,
    span: RefCell<Option<H::Span>>,
    context: Option<TraceContext>,
    error: Cell<Option<ErrorCategory>>,
}

impl<'a, H: WasmTraceHost> WasmSpanAdapter<'a, H> {
    /// Starts a host-owned WIT span.
    #[must_use]
    pub fn new(
        host: &'a H,
        module: Module,
        operation: Operation,
        parent: Option<&TraceContext>,
    ) -> Self {
        let (span, context) = host.start(module, operation, parent);
        Self {
            host,
            span: RefCell::new(Some(span)),
            context,
            error: Cell::new(None),
        }
    }

    /// Returns propagated W3C context for nested component calls.
    #[must_use]
    pub fn context(&self) -> Option<&TraceContext> {
        self.context.as_ref()
    }

    /// Marks a bounded error category for the host-owned span.
    pub fn fail(&self, category: ErrorCategory) {
        self.error.set(Some(category));
    }

    /// Adds one bounded event to the host-owned span.
    pub fn event(&self, event: SignalEvent) {
        if let Some(span) = self.span.borrow().as_ref() {
            self.host.event(span, event);
        }
    }
}

impl<H: WasmTraceHost> Drop for WasmSpanAdapter<'_, H> {
    fn drop(&mut self) {
        if let Some(span) = self.span.get_mut().take() {
            self.host.end(span, self.error.get());
        }
    }
}

impl fmt::Debug for FunctionSpan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FunctionSpan")
            .finish_non_exhaustive()
    }
}

// Per-thread facade state. Each value belongs to the thread that set it, and
// no value is shared across threads (OpenTelemetry providers stay
// process-global; only this bookkeeping is thread-local).
thread_local! {
    // Recorder id of the innermost open observation on this thread; set when
    // an observation starts and restored to its parent's id when it closes.
    static CURRENT_RECORDING: Cell<Option<u64>> = const { Cell::new(None) };
    // Test recorder installed by `install_recorder`; the guard restores the
    // previous value when dropped (last in, first out). Only short
    // `with_borrow`/`with_borrow_mut` accesses touch it, never across a call
    // that could reach it again.
    static RECORDER_SLOT: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

#[cfg(not(target_arch = "wasm32"))]
fn active_recorder() -> Option<Recorder> {
    RECORDER_SLOT.with_borrow(Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use std::sync::atomic::Ordering;

    #[test]
    fn measurement_kinds_have_fixed_instruments_and_units() {
        assert_eq!(
            RuntimeMeasurement::WasmtimeFuel.instrument(),
            ("scxml.runtime.fuel", "{fuel}")
        );
        assert_eq!(
            RuntimeMeasurement::WasmtimeMemoryBytes.instrument(),
            ("scxml.runtime.memory.limit", "By")
        );
        assert_eq!(
            ArtifactMeasurement::SuiteManifestBytes.instrument(),
            ("scxml.artifact.suite-manifest.size", "By")
        );
        assert_eq!(
            ArtifactMeasurement::ReportCases.instrument(),
            ("scxml.artifact.report.cases", "{case}")
        );
    }

    fn child_success() {
        let _span = observe(Module::CompilerCore, Operation::CompilerDecodeIr);
    }

    fn child_error() {
        let span = observe(Module::CompilerCore, Operation::CompilerEncodeIr);
        span.fail(ErrorCategory::Platform);
    }

    #[test]
    fn recorder_closes_success_and_error_with_parent_context() {
        let _span = observe(Module::ConformanceManifest, Operation::ManifestWriteSuite);
        let recorder = Recorder::default();
        let _recorder_guard = install_recorder(recorder.clone());
        {
            let parent = observe(Module::ConformanceManifest, Operation::ManifestLoadSuite);
            parent.event(SignalEvent::InputAccepted);
            parent.event(SignalEvent::ExternalReadCompleted);
            parent.event(SignalEvent::TransformationCompleted);
            child_success();
            child_error();
        }
        let observations = recorder.observations();
        let parent = observations
            .iter()
            .find(|observation| observation.operation == Operation::ManifestLoadSuite)
            .expect("parent observation");
        let success = observations
            .iter()
            .find(|observation| observation.operation == Operation::CompilerDecodeIr)
            .expect("success observation");
        let error = observations
            .iter()
            .find(|observation| observation.operation == Operation::CompilerEncodeIr)
            .expect("error observation");
        assert_eq!(success.parent_id, Some(parent.id));
        assert_eq!(
            parent.events,
            [
                SignalEvent::InputAccepted,
                SignalEvent::ExternalReadCompleted,
                SignalEvent::TransformationCompleted
            ]
        );
        assert_eq!(success.error, None);
        assert_eq!(error.parent_id, Some(parent.id));
        assert_eq!(error.error, Some(ErrorCategory::Platform));
    }

    #[test]
    fn recorders_are_isolated_between_parallel_threads() {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let threads = [Operation::CompilerDecodeIr, Operation::CompilerEncodeIr].map(|operation| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let recorder = Recorder::default();
                let _guard = install_recorder(recorder.clone());
                barrier.wait();
                drop(observe(Module::CompilerCore, operation));
                recorder.observations()
            })
        });

        let observations = threads.map(|thread| {
            thread
                .join()
                .expect("parallel recorder test thread must not panic")
        });
        assert_eq!(observations[0].len(), 1);
        assert_eq!(observations[0][0].operation, Operation::CompilerDecodeIr);
        assert_eq!(observations[1].len(), 1);
        assert_eq!(observations[1][0].operation, Operation::CompilerEncodeIr);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn recorder_id_exhaustion_does_not_panic_wrap_or_record_a_duplicate() {
        let recorder = Recorder::default();
        let _recorder_guard = install_recorder(recorder.clone());
        recorder
            .inner
            .next_id
            .store(u64::MAX - 1, Ordering::Relaxed);

        let observation = std::panic::catch_unwind(|| {
            drop(observe(
                Module::ConformanceManifest,
                Operation::ManifestWriteSuite,
            ));
            drop(observe(
                Module::ConformanceManifest,
                Operation::ManifestWriteSuite,
            ));
        });

        assert!(observation.is_ok(), "recorder ID exhaustion must not panic");
        assert_eq!(recorder.inner.next_id.load(Ordering::Relaxed), u64::MAX);
        let observations = recorder.observations();
        assert_eq!(observations.len(), 1);
        let final_observation = observations
            .first()
            .expect("the final available recorder identifier must be recorded");
        assert_eq!(final_observation.id, u64::MAX);
        assert!(
            recorder
                .inner
                .id_exhaustion_reported
                .load(Ordering::Acquire)
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn recorder_id_allocation_is_collision_free_at_exhaustion() {
        let recorder = Recorder::default();
        recorder
            .inner
            .next_id
            .store(u64::MAX - 1, Ordering::Relaxed);

        let first = recorder.clone();
        let second = recorder.clone();
        let first = std::thread::spawn(move || first.allocate_id());
        let second = std::thread::spawn(move || second.allocate_id());
        let mut allocations = [
            first
                .join()
                .expect("the first allocation worker must not panic"),
            second
                .join()
                .expect("the second allocation worker must not panic"),
        ];
        allocations.sort_by_key(|allocation| match allocation {
            RecorderIdAllocation::Exhausted => 0,
            RecorderIdAllocation::Allocated(_) => 1,
        });

        assert_eq!(
            allocations,
            [
                RecorderIdAllocation::Exhausted,
                RecorderIdAllocation::Allocated(u64::MAX)
            ]
        );
        assert_eq!(recorder.inner.next_id.load(Ordering::Relaxed), u64::MAX);
        assert!(recorder.take_id_exhaustion_signal());
        assert!(!recorder.take_id_exhaustion_signal());
    }

    #[test]
    fn trace_context_accepts_canonical_w3c_carrier() {
        let carrier = TraceContext {
            traceparent: Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned()),
            tracestate: Some("vendor=value".to_owned()),
        };
        let context = extract_trace_context(&carrier).expect("canonical carrier is accepted");
        let encoded = encode_trace_context(&context);
        assert_eq!(encoded, carrier);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn recorder_captures_the_parent_carrier_through_the_public_encoder() {
        let carrier = TraceContext {
            traceparent: Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned()),
            tracestate: Some("vendor=value".to_owned()),
        };
        let parent = extract_trace_context(&carrier).expect("canonical carrier is accepted");
        let recorder = Recorder::default();
        {
            let _guard = install_recorder(recorder.clone());
            drop(start_observation(
                Module::CompilerCore,
                Operation::CompilerDecodeIr,
                &parent,
            ));
            drop(start_observation(
                Module::CompilerCore,
                Operation::CompilerEncodeIr,
                &Context::new(),
            ));
        }
        let observations = recorder.observations();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].parent_trace_context, Some(carrier));
        assert_eq!(
            observations[1].parent_trace_context, None,
            "an absent parent span yields no carrier, not an empty one"
        );
    }

    #[test]
    fn trace_context_rejects_hostile_boundaries() {
        let cases = [
            (
                TraceContext {
                    traceparent: None,
                    tracestate: Some("vendor=value".to_owned()),
                },
                TraceContextError::TraceparentMissing,
            ),
            (
                TraceContext {
                    traceparent: Some(
                        "00-00000000000000000000000000000000-00f067aa0ba902b7-01".to_owned(),
                    ),
                    tracestate: None,
                },
                TraceContextError::TraceparentInvalid,
            ),
            (
                TraceContext {
                    traceparent: Some(
                        "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01".to_owned(),
                    ),
                    tracestate: None,
                },
                TraceContextError::TraceparentInvalid,
            ),
            (
                TraceContext {
                    traceparent: Some(
                        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-02".to_owned(),
                    ),
                    tracestate: None,
                },
                TraceContextError::TraceparentInvalid,
            ),
            (
                TraceContext {
                    traceparent: Some(
                        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned(),
                    ),
                    tracestate: Some("INVALID=value".to_owned()),
                },
                TraceContextError::TracestateInvalid,
            ),
        ];
        for (carrier, expected) in cases {
            assert_eq!(extract_trace_context(&carrier).err(), Some(expected));
        }
        let oversized = TraceContext {
            traceparent: Some("x".repeat(MAX_TRACEPARENT_BYTES + 1)),
            tracestate: None,
        };
        assert_eq!(
            extract_trace_context(&oversized).err(),
            Some(TraceContextError::FieldTooLong)
        );
    }
}
