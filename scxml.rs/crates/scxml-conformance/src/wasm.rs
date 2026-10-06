//! The component execution path: every prepared case runs through the
//! shipped Wasm components under the Wasmtime session host.
//!
//! [`WasmExecutor`] authenticates the release components against a trusted
//! manifest digest, builds one
//! [`SessionHost`](scxml_host_wasmtime::SessionHost) (each component
//! compiled once), and runs each case's lowered Machine IR as the root session: the
//! generic machine plus the data model the document names, the SCXML and
//! `BasicHTTP` I/O processors, the clock, and the compiler for invoked child
//! documents. The runner keeps compilation, capability detection, and
//! every oracle (including the manual-equivalent and external-client
//! oracles); this executor only reports the observations of the real run.
//!
//! # Contract
//!
//! * Preconditions: as [`CaseExecutor`]. `sources` are the case's
//!   integrity-checked prepared dependencies keyed by URI.
//! * Documents and scripts resolve only from `sources`: the exact key, or a
//!   `file:<name>` URI matching a key's final path segment (for invoked
//!   documents also the upstream `.txml` stem). Every prepared payload's
//!   SHA-256 is the script digest allowlist; the prepared manifest already
//!   authenticated those bytes. Nothing touches the network or filesystem.
//! * Postconditions: `commands` and `fuel_consumed` are the engine commands the
//!   run charged (the report's deterministic fuel); `logs` are the root
//!   session's `<log>` records; the trace is the host's session-attributed
//!   observations in order.
//! * Failures map to categories: budget exhaustion and a machine call that
//!   stops at its command bound are `CommandBudget`; a machine session failing
//!   with `unavailable` is `Unsupported`; everything else is `Harness`.
//! * Extension namespaces ([`WasmExecutor::with_extensions`]): each bound
//!   namespace's component, authenticated by its trusted SHA-256, runs its
//!   elements; the run holds exactly the harness's fake values
//!   ([`crate::openai_service::values`]) and its only outbound transport is
//!   the deterministic in-process stand-in
//!   ([`crate::openai_service::OpenAiService`]), reset before every case.
//! * Telemetry: one `run-case-wasm` boundary span per case with closed
//!   labels only.

use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;

use scxml_compiler_core::MachineIr;
use scxml_host_wasmtime::{
    ArtifactKind, DocumentKind, EmbeddedDocument, ExtensionComponents, ExtensionEnvironment,
    ExtensionPackage, HostError, MachineScripts, ResolvedScript, RunFailure, ScriptPolicy,
    ScriptResolveError, ScriptResolver, ScriptedEvent, SessionEmbedder, SessionHost,
    SessionHostComponents, SessionHostError, SessionOutcome, SessionRun, TelemetryProviders,
    Terminal, TraceObservation as HostObservation, TrustedComponentDigest, TrustedManifestDigest,
    WasmtimeHostConfig, authenticate_component, basichttp_types,
};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use sha2::{Digest as _, Sha256};

use crate::executor::{
    BoundExtension, CaseExecutor, CaseSnapshotDocument, ExecutionError, ExecutionFailureCategory,
    ExecutionResult, ExecutionTrace, ExecutorIdentities, ExternalClientRun, HttpHeader,
    HttpRequest, HttpResponse, LoadRejection, LogRecord, TraceObservation, TraceRecord,
    implementation_identity,
};
use crate::report::{RUNNER_VERSION, TerminalStatus};

/// `_sessionid` of every root session the executor runs.
pub const ROOT_SESSION_ID: &str = "conformance";

/// Release component artifacts the executor links, by manifest file name.
const COMPONENTS: [(ArtifactKind, &str); 8] = [
    (ArtifactKind::Machine, "scxml_machine.virtualized.wasm"),
    (ArtifactKind::Compiler, "scxml_compiler.virtualized.wasm"),
    (
        ArtifactKind::DataModelEcmascript,
        "scxml_data_model_ecmascript.virtualized.wasm",
    ),
    (
        ArtifactKind::DataModelXpath,
        "scxml_data_model_xpath.virtualized.wasm",
    ),
    (
        ArtifactKind::DataModelNull,
        "scxml_data_model_null.virtualized.wasm",
    ),
    (
        ArtifactKind::IoProcessorScxml,
        "scxml_ioprocessor_scxml.virtualized.wasm",
    ),
    (
        ArtifactKind::IoProcessorBasicHttp,
        "scxml_ioprocessor_basichttp.virtualized.wasm",
    ),
    (ArtifactKind::Clock, "scxml_clock.virtualized.wasm"),
];

/// Byte bound of the release `manifest.tsv`: the artifact authenticator's
/// own manifest bound (64 KiB), enforced here before the read so an
/// oversized manifest is never loaded.
const MAX_RELEASE_MANIFEST_BYTES: u64 = 64 * 1024;

/// Byte bound of one extension package manifest file (the CLI's bound).
const MAX_EXTENSION_MANIFEST_BYTES: u64 = 64 * 1024;

/// Why a bounded read failed.
enum ReadFailure {
    /// The file could not be opened, inspected, or read.
    Io(std::io::Error),
    /// The file is not regular or exceeds its bound; nothing past the bound
    /// was read.
    Oversized,
}

/// Reads `path` only when it is a regular file of at most `limit` bytes,
/// reading at most `limit + 1` bytes so a file that grows after the metadata
/// check is still rejected without being loaded.
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, ReadFailure> {
    use std::io::Read as _;
    // Opening a FIFO or a device can block: check the path first, and the
    // handle again below.
    if !std::fs::metadata(path).map_err(ReadFailure::Io)?.is_file() {
        return Err(ReadFailure::Oversized);
    }
    let file = std::fs::File::open(path).map_err(ReadFailure::Io)?;
    let metadata = file.metadata().map_err(ReadFailure::Io)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(ReadFailure::Oversized);
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| ReadFailure::Oversized)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(ReadFailure::Io)?;
    if u64::try_from(bytes.len()).map_or(true, |read| read > limit) {
        return Err(ReadFailure::Oversized);
    }
    Ok(bytes)
}

/// Reads the release file `name` in `dir` ([`read_bounded`]).
fn read_release_file(
    dir: &Path,
    name: &'static str,
    limit: u64,
) -> Result<Vec<u8>, WasmExecutorError> {
    read_bounded(&dir.join(name), limit).map_err(|failure| match failure {
        ReadFailure::Io(error) => WasmExecutorError::Read(name, error),
        ReadFailure::Oversized => WasmExecutorError::Oversized(name, limit),
    })
}

impl ExtensionRelease {
    /// Reads one extension namespace: the `stateforward.scxml.extension-package/v2`
    /// manifest at `manifest`, its schema (`schema-artifact`, beside the
    /// manifest), and its component (`extension-artifact`) from the release
    /// directory `components`, each a bounded regular file, with the
    /// component's trusted SHA-256 `component_sha256`. Nothing is verified
    /// until [`WasmExecutor::with_extensions`].
    ///
    /// # Errors
    ///
    /// [`WasmExecutorError::ExtensionFile`] for an unreadable, oversized, or
    /// malformed file, or an artifact name that is not a plain file name.
    pub fn read(
        manifest: &Path,
        components: &Path,
        component_sha256: &str,
    ) -> Result<Self, WasmExecutorError> {
        let failure = |path: &Path, message: String| {
            WasmExecutorError::ExtensionFile(path.display().to_string(), message)
        };
        let read = |path: &Path, limit: u64| {
            read_bounded(path, limit).map_err(|error| match error {
                ReadFailure::Io(error) => failure(path, error.to_string()),
                ReadFailure::Oversized => {
                    failure(path, format!("not a regular file of at most {limit} bytes"))
                }
            })
        };
        let manifest_bytes = read(manifest, MAX_EXTENSION_MANIFEST_BYTES)?;
        let parsed = scxml_validator_core::ExtensionPackageManifest::from_json(&manifest_bytes)
            .map_err(|error| failure(manifest, error))?;
        let file_name = |name: &str| -> Result<std::path::PathBuf, WasmExecutorError> {
            let path = Path::new(name);
            (path.components().count() == 1
                && matches!(
                    path.components().next(),
                    Some(std::path::Component::Normal(_))
                ))
            .then(|| path.to_path_buf())
            .ok_or_else(|| failure(manifest, "an artifact must be a plain file name".into()))
        };
        let schema_path = manifest
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(file_name(&parsed.schema_artifact)?);
        let schema_limit =
            u64::try_from(scxml_validator_core::MAX_COMPONENT_SCHEMA_BYTES).unwrap_or(u64::MAX);
        let schema = scxml_validator_core::SchemaSource::new(
            parsed.schema_artifact.clone(),
            read(&schema_path, schema_limit)?,
        );
        let component_limit = u64::try_from(scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES)
            .unwrap_or(u64::MAX);
        let component = read(
            &components.join(file_name(&parsed.extension_artifact)?),
            component_limit,
        )?;
        Ok(Self {
            manifest: parsed,
            manifest_sha256: hex::encode(Sha256::digest(&manifest_bytes)),
            schema,
            component,
            component_sha256: component_sha256.to_owned(),
        })
    }
}

/// One extension namespace to bind: its verified package and its
/// extension component's bytes with their trusted SHA-256.
#[derive(Clone, Debug)]
pub struct ExtensionRelease {
    /// The package manifest (verified against `schema` when bound).
    pub manifest: scxml_validator_core::ExtensionPackageManifest,
    /// The lowercase SHA-256 of the exact manifest bytes it was read from.
    pub manifest_sha256: String,
    /// The schema bytes the manifest's digest names.
    pub schema: scxml_validator_core::SchemaSource,
    /// The extension component bytes.
    pub component: Vec<u8>,
    /// The trusted lowercase SHA-256 of `component`.
    pub component_sha256: String,
}

/// Failure to construct a [`WasmExecutor`].
#[derive(Debug)]
pub enum WasmExecutorError {
    /// The trusted manifest digest was not 64 lowercase hex characters.
    InvalidDigest,
    /// An extension package does not verify against its schema (the
    /// namespace URI and the verifier's message).
    ExtensionPackage(String, String),
    /// An extension package file could not be read or parsed (its path and
    /// the reason).
    ExtensionFile(String, String),
    /// A release file could not be read (its file name).
    Read(&'static str, std::io::Error),
    /// A release file is not a regular file or exceeds its byte bound (its
    /// file name and the bound); nothing past the bound was read.
    Oversized(&'static str, u64),
    /// Authentication, validation, or compilation failed.
    Host(HostError),
}

impl std::fmt::Display for WasmExecutorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDigest => {
                formatter.write_str("the component manifest digest must be 64 lowercase hex")
            }
            Self::Read(name, error) => write!(formatter, "cannot read {name}: {error}"),
            Self::Oversized(name, limit) => write!(
                formatter,
                "{name} must be a regular file of at most {limit} bytes"
            ),
            Self::ExtensionFile(path, error) => write!(formatter, "cannot read `{path}`: {error}"),
            Self::ExtensionPackage(namespace, error) => write!(
                formatter,
                "the extension package of `{namespace}` does not verify: {error}"
            ),
            Self::Host(error) => write!(formatter, "component host rejected the release: {error}"),
        }
    }
}

impl std::error::Error for WasmExecutorError {}

/// Whether a [`WasmExecutor`] collects session-snapshot documents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SnapshotCollection {
    /// Collect nothing (every conformance run but the snapshot proof).
    Off,
    /// One document per reached checkpoint, on pinned telemetry IDs.
    EveryCheckpoint,
}

/// Runs compiled cases through the shipped components under Wasmtime.
/// Single-threaded (`!Send`): create one per thread.
pub struct WasmExecutor {
    host: SessionHost,
    identities: ExecutorIdentities,
    /// Whether to collect one read-only session-snapshot document per
    /// reached checkpoint (docs/session-snapshot.md, "Non-perturbation proof
    /// over the IRP"). Collection never perturbs a run: the snapshots ride
    /// the same `SessionRun` and the oracle never reads them.
    snapshot_collection: SnapshotCollection,
    /// The bound extension namespaces (namespace-URI order).
    extensions: Vec<BoundExtension>,
    /// The outbound transport of the bound extension components, when any.
    service: Option<std::sync::Arc<crate::openai_service::OpenAiService>>,
}

impl std::fmt::Debug for WasmExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WasmExecutor")
            .finish_non_exhaustive()
    }
}

impl WasmExecutor {
    /// Authenticates the release components in `dir` (`manifest.tsv` plus
    /// the `*.virtualized.wasm` artifacts written by
    /// `scripts/verify-wasm-capabilities.sh`) against
    /// `trusted_manifest_sha256` and compiles them once.
    ///
    /// # Errors
    ///
    /// [`WasmExecutorError`] for an invalid digest, an unreadable file, a
    /// file that is not regular or exceeds its bound (64 KiB for the
    /// manifest, [`MAX_AUTHENTICATED_COMPONENT_BYTES`] per component; checked
    /// before the file is read), or any authentication or compilation
    /// failure; nothing is returned.
    ///
    /// [`MAX_AUTHENTICATED_COMPONENT_BYTES`]: scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES
    pub fn from_release_dir(
        dir: &Path,
        trusted_manifest_sha256: &str,
    ) -> Result<Self, WasmExecutorError> {
        Self::build(dir, trusted_manifest_sha256, SnapshotCollection::Off)
    }

    /// Builds the executor with snapshot collection: every
    /// run also gathers one read-only session-snapshot document per reached
    /// checkpoint (nothing excluded, `preserve`), on telemetry providers
    /// whose IDs are pinned so documents are byte-stable across runs.
    ///
    /// # Errors
    ///
    /// As [`WasmExecutor::from_release_dir`].
    pub fn from_release_dir_collecting_snapshots(
        dir: &Path,
        trusted_manifest_sha256: &str,
    ) -> Result<Self, WasmExecutorError> {
        Self::build(
            dir,
            trusted_manifest_sha256,
            SnapshotCollection::EveryCheckpoint,
        )
    }

    /// The shared constructor; `snapshot_collection` selects both the
    /// capture behavior and the pinned telemetry providers it needs.
    fn build(
        dir: &Path,
        trusted_manifest_sha256: &str,
        snapshot_collection: SnapshotCollection,
    ) -> Result<Self, WasmExecutorError> {
        let trusted = TrustedManifestDigest::from_hex(trusted_manifest_sha256)
            .map_err(|_| WasmExecutorError::InvalidDigest)?;
        let manifest = read_release_file(dir, "manifest.tsv", MAX_RELEASE_MANIFEST_BYTES)?;
        let component_limit = u64::try_from(scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES)
            .unwrap_or(u64::MAX);
        let mut authenticated = Vec::with_capacity(COMPONENTS.len());
        for (kind, name) in COMPONENTS {
            let bytes = read_release_file(dir, name, component_limit)?;
            let component = authenticate_component(&manifest, trusted, kind, name, &bytes)
                .map_err(WasmExecutorError::Host)?;
            authenticated.push((kind, component, bytes));
        }
        let find = |kind: ArtifactKind| {
            authenticated
                .iter()
                .find(|(candidate, ..)| *candidate == kind)
                .map(|(_, component, bytes)| (component, bytes.as_slice()))
                .ok_or(WasmExecutorError::Host(HostError::Artifact(
                    scxml_host_wasmtime::ArtifactError::ArtifactIdentity,
                )))
        };
        let machine = find(ArtifactKind::Machine)?;
        let ecmascript = find(ArtifactKind::DataModelEcmascript)?;
        let xpath = find(ArtifactKind::DataModelXpath)?;
        let null = find(ArtifactKind::DataModelNull)?;
        let components = SessionHostComponents {
            machine: machine.0,
            compiler: find(ArtifactKind::Compiler)?.0,
            data_model_ecmascript: ecmascript.0,
            data_model_xpath: xpath.0,
            data_model_null: null.0,
            io_scxml: find(ArtifactKind::IoProcessorScxml)?.0,
            io_basichttp: find(ArtifactKind::IoProcessorBasicHttp)?.0,
            clock: find(ArtifactKind::Clock)?.0,
        };
        let config = WasmtimeHostConfig::default();
        let host = if snapshot_collection == SnapshotCollection::EveryCheckpoint {
            // Under snapshot collection every document must be byte-stable
            // across runs, so the pinned sequential ID generator replaces
            // the random one; reports never observe
            // telemetry either way.
            let providers = TelemetryProviders::deterministic();
            SessionHost::new(&components, &config, &providers)
        } else {
            let providers = TelemetryProviders::new();
            SessionHost::new(&components, &config, &providers)
        }
        .map_err(WasmExecutorError::Host)?;
        let identity = |name: &'static str, bytes: &[u8]| {
            implementation_identity(name, RUNNER_VERSION, &[bytes])
        };
        let identities = ExecutorIdentities {
            engine: identity("scxml-machine", machine.1),
            runner: implementation_identity(
                "scxml-conformance/wasm",
                RUNNER_VERSION,
                &[
                    include_bytes!("lib.rs"),
                    include_bytes!("capability.rs"),
                    include_bytes!("errata.rs"),
                    include_bytes!("executor.rs"),
                    include_bytes!("external_client.rs"),
                    include_bytes!("filter.rs"),
                    include_bytes!("manual_oracle.rs"),
                    include_bytes!("openai_service.rs"),
                    include_bytes!("oracle.rs"),
                    include_bytes!("report.rs"),
                    include_bytes!("runner.rs"),
                    include_bytes!("wasm.rs"),
                ],
            ),
            data_models: [
                (
                    "ecmascript",
                    identity("scxml-data-model-ecmascript", ecmascript.1),
                ),
                ("xpath", identity("scxml-data-model-xpath", xpath.1)),
                ("null", identity("scxml-data-model-null", null.1)),
            ]
            .into_iter()
            .map(|(name, identity)| (name.to_owned(), identity))
            .collect(),
            unrouted_data_model: identity("scxml-machine-unlinked-datamodel", machine.1),
        };
        Ok(Self {
            host,
            identities,
            snapshot_collection,
            extensions: Vec::new(),
            service: None,
        })
    }

    /// Binds `releases` (each namespace at most once): every component is
    /// authenticated by its trusted SHA-256 and every package verified
    /// against its schema; the run then holds the harness's fake values
    /// (granted to every bound namespace) and
    /// reaches only the in-process stand-in
    /// ([`crate::openai_service`]). Replaces any earlier binding.
    ///
    /// # Errors
    ///
    /// [`WasmExecutorError::InvalidDigest`] for a malformed digest,
    /// [`WasmExecutorError::ExtensionPackage`] for a package that does not
    /// verify, and [`WasmExecutorError::Host`] for an authentication or
    /// binding failure.
    pub fn with_extensions(
        mut self,
        releases: Vec<ExtensionRelease>,
    ) -> Result<Self, WasmExecutorError> {
        let service = std::sync::Arc::new(crate::openai_service::OpenAiService::new());
        // The fake values are granted to every namespace this run binds.
        let uris: Vec<String> = releases
            .iter()
            .map(|release| release.manifest.namespace_uri.clone())
            .collect();
        let values = if uris.is_empty() {
            ExtensionEnvironment::none()
        } else {
            ExtensionEnvironment::new(crate::openai_service::values().into_iter().map(
                |(name, value)| {
                    scxml_host_wasmtime::EnvVariable::new(name, value, uris.iter().cloned())
                },
            ))
            .map_err(WasmExecutorError::Host)?
        };
        let mut components = ExtensionComponents::new()
            .with_values(values)
            .with_outbound_http(service.clone());
        let mut bound = Vec::with_capacity(releases.len());
        for release in releases {
            let manifest = release.manifest;
            manifest.verify_schema(&release.schema).map_err(|error| {
                WasmExecutorError::ExtensionPackage(manifest.namespace_uri.clone(), error)
            })?;
            let trusted = TrustedComponentDigest::from_hex(&release.component_sha256)
                .map_err(|_| WasmExecutorError::InvalidDigest)?;
            let component =
                scxml_host_wasmtime::authenticate_extension_component(&release.component, trusted)
                    .map_err(WasmExecutorError::Host)?;
            components = components
                .bind(
                    ExtensionPackage {
                        namespace_uri: manifest.namespace_uri.clone(),
                        package_version: manifest.package_version.clone(),
                        extension_artifact: manifest.extension_artifact.clone(),
                        schema_artifact: manifest.schema_artifact.clone(),
                        schema_sha256: manifest.schema_sha256.clone(),
                        wit_world: manifest.wit_world.clone(),
                        wit_version: manifest.wit_version.clone(),
                        env_slots: manifest.env_slots.clone(),
                        http_origins: manifest.http_origins.clone(),
                        wasi_permissions: manifest.wasi_permissions.clone(),
                        schema: release.schema.bytes.clone(),
                    },
                    &component,
                )
                .map_err(WasmExecutorError::Host)?;
            bound.push(BoundExtension {
                identity: scxml_reporting::ConformanceExtensionIdentity {
                    namespace_uri: manifest.namespace_uri.clone(),
                    package_version: manifest.package_version.clone(),
                    manifest_sha256: release.manifest_sha256.clone(),
                    schema_sha256: manifest.schema_sha256.clone(),
                    component_sha256: hex::encode(Sha256::digest(&release.component)),
                },
                manifest,
                schema: release.schema,
            });
        }
        bound.sort_by(|left, right| {
            left.manifest
                .namespace_uri
                .cmp(&right.manifest.namespace_uri)
        });
        self.host = self
            .host
            .with_extension_components(components)
            .map_err(WasmExecutorError::Host)?;
        self.extensions = bound;
        self.service = Some(service);
        Ok(self)
    }

    fn run(
        &self,
        ir: &MachineIr,
        ir_bytes: &[u8],
        events: &[String],
        external_client: Option<&dyn Fn(&str) -> basichttp_types::HttpRequest>,
        max_commands: u64,
        sources: &BTreeMap<String, String>,
    ) -> Result<SessionOutcome, ExecutionError> {
        let span = observe(
            Module::ConformanceRunner,
            Operation::ConformanceRunnerRunCaseWasm,
        );
        if let Some(service) = &self.service {
            service.reset();
        }
        let embedder: Rc<dyn SessionEmbedder> = Rc::new(PreparedSources {
            sources: sources.clone(),
        });
        let events: Vec<ScriptedEvent> = events.iter().map(ScriptedEvent::named).collect();
        let result = self.host.run(&SessionRun {
            machine_ir: ir_bytes,
            data_model: ir.data_model.as_deref(),
            session_id: ROOT_SESSION_ID,
            events: &events,
            external_client,
            embedder,
            max_commands,
            snapshots: if self.snapshot_collection == SnapshotCollection::EveryCheckpoint {
                scxml_host_wasmtime::SnapshotRequest {
                    points: scxml_host_wasmtime::SnapshotPoints::Every,
                    options: scxml_snapshot::Options {
                        exclude: Vec::new(),
                        trace_context: scxml_snapshot::TraceContextPolicy::Preserve,
                        point: scxml_snapshot::SnapshotPoint::Checkpoint,
                    },
                }
            } else {
                scxml_host_wasmtime::SnapshotRequest::none()
            },
        });
        match result {
            Ok(outcome) => {
                if let Some(error) = snapshot_collection_failure(&outcome) {
                    span.event(SignalEvent::InputRejected);
                    span.fail(ErrorCategory::Execution);
                    return Err(error);
                }
                span.event(SignalEvent::TransformationCompleted);
                Ok(outcome)
            }
            Err(error) => {
                let error = execution_error(error, max_commands);
                span.event(SignalEvent::InputRejected);
                span.fail(match error.category {
                    ExecutionFailureCategory::CommandBudget => ErrorCategory::Budget,
                    ExecutionFailureCategory::Unsupported => ErrorCategory::Unsupported,
                    ExecutionFailureCategory::Harness => ErrorCategory::Execution,
                });
                Err(error)
            }
        }
    }
}

impl CaseExecutor for WasmExecutor {
    fn identities(&self) -> ExecutorIdentities {
        self.identities.clone()
    }

    fn collects_snapshots(&self) -> bool {
        self.snapshot_collection == SnapshotCollection::EveryCheckpoint
    }

    fn extensions(&self) -> Vec<BoundExtension> {
        self.extensions.clone()
    }

    fn execute(
        &self,
        ir_bytes: &[u8],
        ir: &MachineIr,
        events: &[String],
        max_commands: u64,
        sources: &BTreeMap<String, String>,
    ) -> Result<ExecutionResult, ExecutionError> {
        self.run(ir, ir_bytes, events, None, max_commands, sources)
            .map(execution_result)
    }

    fn execute_external_client(
        &self,
        ir_bytes: &[u8],
        ir: &MachineIr,
        request: &dyn Fn(&str) -> HttpRequest,
        max_commands: u64,
        sources: &BTreeMap<String, String>,
    ) -> Result<ExternalClientRun, ExecutionError> {
        let client = |location: &str| host_request(request(location));
        let outcome = self.run(ir, ir_bytes, &[], Some(&client), max_commands, sources)?;
        if matches!(outcome.terminal, Terminal::Rejected(_)) {
            return Err(ExecutionError::new(
                ExecutionFailureCategory::Harness,
                "external-client-receiver-rejected",
                "the harness-authored receiving document was rejected at load",
            ));
        }
        let Some(response) = outcome.http_response.clone() else {
            return Err(ExecutionError::new(
                ExecutionFailureCategory::Harness,
                "external-client-no-response",
                "the external client received no response",
            ));
        };
        Ok(ExternalClientRun {
            execution: execution_result(outcome),
            response: HttpResponse {
                status: response.status,
                headers: response
                    .headers
                    .into_iter()
                    .map(|header| HttpHeader {
                        name: header.name,
                        value: header.value,
                    })
                    .collect(),
                body: response.body,
            },
        })
    }
}

fn host_request(request: HttpRequest) -> basichttp_types::HttpRequest {
    basichttp_types::HttpRequest {
        method: request.method,
        uri: request.uri,
        headers: request
            .headers
            .into_iter()
            .map(|header| basichttp_types::HttpHeader {
                name: header.name,
                value: header.value,
            })
            .collect(),
        body: request.body,
    }
}

fn log_record(label: Option<String>, text: String) -> LogRecord {
    LogRecord {
        label: label.unwrap_or_default(),
        value: text,
    }
}

/// Projects one completed run onto the runner's observations.
fn execution_result(outcome: SessionOutcome) -> ExecutionResult {
    let terminal_status = match outcome.terminal {
        Terminal::Running => TerminalStatus::Running,
        Terminal::Finished => TerminalStatus::Finished,
        Terminal::Rejected(rejection) => {
            return ExecutionResult {
                configuration: Vec::new(),
                terminal_status: TerminalStatus::Rejected,
                commands: 0,
                fuel_consumed: 0,
                logs: Vec::new(),
                trace: ExecutionTrace::default(),
                load_rejection: Some(match rejection {
                    scxml_host_wasmtime::LoadRejection::ScriptUnavailable => {
                        LoadRejection::ScriptUnavailable
                    }
                    scxml_host_wasmtime::LoadRejection::ScriptInvalid => {
                        LoadRejection::ScriptInvalid
                    }
                }),
                snapshots: Vec::new(),
            };
        }
    };
    let records = outcome
        .trace
        .into_iter()
        .map(|entry| TraceRecord {
            session: entry.session,
            parent_session: entry.parent,
            observation: match entry.observation {
                HostObservation::Command => TraceObservation::Command,
                HostObservation::Raised(event) => TraceObservation::Raise { event },
                HostObservation::Processed(event) => TraceObservation::Processed { event },
                HostObservation::Log(log) => TraceObservation::Log(log_record(log.label, log.text)),
            },
        })
        .collect();
    ExecutionResult {
        configuration: outcome.configuration,
        terminal_status,
        commands: outcome.commands,
        fuel_consumed: outcome.commands,
        logs: outcome
            .root_logs
            .into_iter()
            .map(|log| log_record(log.label, log.text))
            .collect(),
        trace: ExecutionTrace { records },
        load_rejection: None,
        snapshots: outcome
            .snapshots
            .into_iter()
            .map(|snapshot| CaseSnapshotDocument {
                checkpoint: snapshot.checkpoint,
                document: snapshot.bytes,
            })
            .collect(),
    }
}

/// Collection fails closed: a run whose checkpoint snapshots did not all
/// succeed (the host records each failure and lets the session go on) is a
/// harness failure, never a run with a gap in its snapshots. Without
/// collection the host takes no snapshot and records no failure.
fn snapshot_collection_failure(outcome: &SessionOutcome) -> Option<ExecutionError> {
    let first = outcome.snapshot_failures.first()?;
    let mut failure = ExecutionError::new(
        ExecutionFailureCategory::Harness,
        "session-snapshot",
        format!(
            "{} checkpoint snapshot(s) failed; checkpoint {}: {}",
            outcome.snapshot_failures.len(),
            first.checkpoint,
            first.failure
        ),
    );
    failure.commands = outcome.commands;
    failure.fuel_consumed = outcome.commands;
    Some(failure)
}

/// Maps one failed run onto the runner's failure categories.
fn execution_error(error: SessionHostError, max_commands: u64) -> ExecutionError {
    let (category, code, message) = match error.failure {
        RunFailure::CommandBudget => (
            ExecutionFailureCategory::CommandBudget,
            "command-budget-exceeded",
            format!("execution exceeded max_commands={max_commands}"),
        ),
        RunFailure::CommandBound => (
            ExecutionFailureCategory::CommandBudget,
            "engine-run-bound",
            "the engine stopped at its per-run command bound before a stable configuration".into(),
        ),
        RunFailure::Machine { code, detail } => {
            let category = if code == "unavailable" {
                ExecutionFailureCategory::Unsupported
            } else {
                ExecutionFailureCategory::Harness
            };
            let message = format!(
                "machine session failed ({code}): {}",
                detail.as_deref().unwrap_or("no detail")
            );
            let fallback = if category == ExecutionFailureCategory::Unsupported {
                "machine-capability-unavailable"
            } else {
                "machine-session-failed"
            };
            (category, fallback, message)
        }
        RunFailure::Harness { code, message } => (ExecutionFailureCategory::Harness, code, message),
        RunFailure::Host(host) => (
            ExecutionFailureCategory::Harness,
            "wasm-host",
            host.to_string(),
        ),
        RunFailure::Snapshot(failure) => (
            ExecutionFailureCategory::Harness,
            "session-snapshot",
            failure.to_string(),
        ),
    };
    let mut failure = ExecutionError::new(category, code, message);
    failure.commands = error.commands;
    failure.fuel_consumed = error.commands;
    failure
}

/// The case's prepared dependencies as the run's documents and scripts.
struct PreparedSources {
    sources: BTreeMap<String, String>,
}

fn file_name(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

impl PreparedSources {
    /// The exact key, else a `file:<name>` URI matching a key's final path
    /// segment.
    fn resolve(&self, uri: &str) -> Option<(&str, &str)> {
        if let Some((key, text)) = self.sources.get_key_value(uri) {
            return Some((file_name(key), text));
        }
        let name = uri.strip_prefix("file:")?;
        self.sources
            .iter()
            .map(|(key, text)| (file_name(key), text.as_str()))
            .find(|(key_name, _)| *key_name == name)
    }
}

impl SessionEmbedder for PreparedSources {
    /// Prepared sources carry no media type: the official cases name their
    /// documents by `file:` URI and expect them read by content.
    fn document(&self, kind: DocumentKind, uri: &str) -> Option<EmbeddedDocument> {
        let text = match kind {
            DocumentKind::Data => self.resolve(uri).map(|(_, text)| text.to_owned()),
            DocumentKind::Invocation => {
                // Prepared keys may carry the upstream `.txml` suffix while
                // `src` names the transformed document; the transformed
                // `.scxml` dependency wins.
                let file = uri.strip_prefix("file:").unwrap_or(uri);
                let stem = file.strip_suffix(".scxml").unwrap_or(file);
                self.sources
                    .iter()
                    .find(|(key, _)| file_name(key) == file)
                    .or_else(|| {
                        self.sources
                            .iter()
                            .find(|(key, _)| file_name(key).strip_suffix(".txml") == Some(stem))
                    })
                    .map(|(_, text)| text.clone())
            }
        };
        text.map(EmbeddedDocument::from)
    }

    fn scripts(&self) -> MachineScripts {
        let trusted = self.sources.values().filter_map(|text| {
            TrustedComponentDigest::from_hex(&hex::encode(Sha256::digest(text.as_bytes()))).ok()
        });
        MachineScripts::runtime(
            Box::new(SourceScripts {
                sources: self.sources.clone(),
            }),
            ScriptPolicy::new(trusted),
        )
    }
}

/// The runtime `script-loader` over the prepared dependencies. The media
/// type follows the resolved file name: `.js`/`.mjs` are
/// `text/javascript`, `.wasm` is `application/wasm`; any other name
/// declares none and fails authentication.
struct SourceScripts {
    sources: BTreeMap<String, String>,
}

impl ScriptResolver for SourceScripts {
    fn resolve(&mut self, uri: &str) -> Result<ResolvedScript, ScriptResolveError> {
        let prepared = PreparedSources {
            sources: std::mem::take(&mut self.sources),
        };
        let resolved = prepared
            .resolve(uri)
            .map(|(name, text)| (name.to_owned(), text.as_bytes().to_vec()));
        self.sources = prepared.sources;
        let (name, bytes) = resolved.ok_or(ScriptResolveError::NotFound)?;
        let media_type = match name.rsplit_once('.').map(|(_, extension)| extension) {
            Some("js" | "mjs") => "text/javascript",
            Some("wasm") => scxml_script::WASM_MEDIA_TYPE,
            _ => "application/octet-stream",
        };
        Ok(ResolvedScript {
            media_type: media_type.into(),
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_collection_fails_closed_on_any_failed_checkpoint() {
        let mut outcome = SessionOutcome {
            terminal: Terminal::Finished,
            configuration: Vec::new(),
            commands: 7,
            root_logs: Vec::new(),
            trace: Vec::new(),
            http_response: None,
            snapshots: Vec::new(),
            snapshot_failures: Vec::new(),
        };
        assert_eq!(snapshot_collection_failure(&outcome), None);
        outcome
            .snapshot_failures
            .push(scxml_host_wasmtime::CheckpointSnapshotFailure {
                checkpoint: 3,
                failure: scxml_host_wasmtime::SnapshotFailure::InvalidPoints,
            });
        let failure =
            snapshot_collection_failure(&outcome).expect("a failed snapshot fails closed");
        assert_eq!(failure.category, ExecutionFailureCategory::Harness);
        assert_eq!(failure.code, "session-snapshot");
        assert_eq!((failure.commands, failure.fuel_consumed), (7, 7));
        assert!(
            failure.message.contains("checkpoint 3"),
            "{}",
            failure.message
        );
    }

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// A fresh, empty directory removed on drop.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "scxml-conformance-release-{label}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("the scratch release directory is creatable");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Writes a sparse file of `len` bytes: no data is allocated, so only a
    /// read would cost memory.
    fn sparse(path: &Path, len: u64) {
        std::fs::File::create(path)
            .and_then(|file| file.set_len(len))
            .expect("the sparse fixture is writable");
    }

    fn rejection(dir: &Path) -> WasmExecutorError {
        WasmExecutor::from_release_dir(dir, DIGEST).expect_err("the release is rejected")
    }

    #[test]
    fn an_oversized_manifest_is_rejected_before_it_is_read() {
        let release = Scratch::new("oversized-manifest");
        sparse(&release.0.join("manifest.tsv"), 1 << 30);
        assert!(matches!(
            rejection(&release.0),
            WasmExecutorError::Oversized("manifest.tsv", MAX_RELEASE_MANIFEST_BYTES)
        ));
        sparse(
            &release.0.join("manifest.tsv"),
            MAX_RELEASE_MANIFEST_BYTES + 1,
        );
        assert!(matches!(
            rejection(&release.0),
            WasmExecutorError::Oversized("manifest.tsv", _)
        ));
    }

    #[test]
    fn a_non_regular_manifest_is_rejected() {
        let release = Scratch::new("directory-manifest");
        std::fs::create_dir(release.0.join("manifest.tsv"))
            .expect("the directory fixture is creatable");
        assert!(matches!(
            rejection(&release.0),
            WasmExecutorError::Oversized("manifest.tsv", _)
        ));
    }

    #[test]
    fn an_oversized_or_non_regular_component_is_rejected_before_it_is_read() {
        let release = Scratch::new("oversized-component");
        std::fs::write(release.0.join("manifest.tsv"), b"within the bound\n")
            .expect("the manifest fixture is writable");
        let (_, first) = COMPONENTS[0];
        let limit = u64::try_from(scxml_host_wasmtime::MAX_AUTHENTICATED_COMPONENT_BYTES)
            .expect("the component bound fits u64");
        sparse(&release.0.join(first), limit + 1);
        assert!(matches!(
            rejection(&release.0),
            WasmExecutorError::Oversized(name, bound) if name == first && bound == limit
        ));
        std::fs::remove_file(release.0.join(first)).expect("the fixture is removable");
        std::fs::create_dir(release.0.join(first)).expect("the directory fixture is creatable");
        assert!(matches!(
            rejection(&release.0),
            WasmExecutorError::Oversized(name, _) if name == first
        ));
    }

    #[test]
    fn an_unavailable_machine_capability_maps_to_unsupported() {
        let failure = execution_error(
            SessionHostError {
                failure: RunFailure::Machine {
                    code: "unavailable",
                    detail: Some("another cause".into()),
                },
                commands: 0,
            },
            100,
        );
        assert_eq!(failure.category, ExecutionFailureCategory::Unsupported);
        assert_eq!(failure.code, "machine-capability-unavailable");
    }
}
