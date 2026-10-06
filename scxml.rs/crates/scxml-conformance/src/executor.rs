//! Execution seam of the conformance runner and the observations it returns.
//!
//! The runner owns everything that decides a case: corpus loading, filters,
//! the canonical compiler, capability detection, and every oracle. A
//! [`CaseExecutor`] owns only how one compiled case executes. The one
//! execution path is the shipped Wasm components under the Wasmtime session
//! host (`crate::wasm::WasmExecutor`, behind the `wasmtime` feature); this
//! crate links no in-process SCXML execution.
//!
//! # Contract
//!
//! * Preconditions: the runner calls an executor only for a case whose source
//!   the canonical compiler accepted and lowered; `ir_bytes` are exactly the
//!   compiler's `LoweredIr` artifact bytes and `ir` is their decoded form.
//!   `max_commands` is the case's manifest command budget; `sources` are the case's
//!   integrity-checked UTF-8 dependency payloads keyed by URI.
//! * Postconditions: an executor returns observations of the real run
//!   ([`ExecutionResult`]) or a typed [`ExecutionError`] whose category maps
//!   to a truthful disposition; it never decides pass or fail.
//! * Invariants: an executor never parses SCXML, never selects transitions
//!   outside `scxml-engine`, and never substitutes final-state guesses for
//!   engine behavior (AGENTS.md). [`CaseExecutor::identities`] names exactly
//!   the implementation that produced the observations.
//! * Side effects and bounds: none beyond the bounded execution itself; each
//!   call is independent and owns its sessions for its duration.

use std::collections::BTreeMap;
use std::fmt;

use scxml_compiler_core::MachineIr;
use scxml_reporting::ConformanceImplementationIdentity;
use sha2::{Digest as _, Sha256};

use crate::report::TerminalStatus;

pub use scxml_ioprocessor_basichttp::{HttpHeader, HttpRequest, HttpResponse};

#[derive(Clone, Debug, Eq, PartialEq)]
/// One observable SCXML `log` effect emitted during execution.
pub struct LogRecord {
    /// Evaluated log label.
    pub label: String,
    /// Evaluated log value.
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Deterministic observations produced by a successful engine run.
pub struct ExecutionResult {
    /// Active state identifiers in engine-defined deterministic order.
    pub configuration: Vec<String>,
    /// Whether the machine remains running or has finished.
    pub terminal_status: TerminalStatus,
    /// Engine commands charged to the command budget.
    pub commands: u64,
    /// Deterministic fuel charged by the executor.
    pub fuel_consumed: u64,
    /// Ordered log effects emitted by the root session's executable content;
    /// the only input of the official `Outcome` oracle.
    pub logs: Vec<LogRecord>,
    /// Ordered cross-session observations of the whole run, consumed only by
    /// the documented manual-equivalent oracles (`crate::manual_oracle`).
    pub trace: ExecutionTrace,
    /// Why the processor rejected the document at load; `Some` exactly when
    /// `terminal_status` is [`TerminalStatus::Rejected`], in which case no
    /// session ran and every other observation is empty.
    pub load_rejection: Option<LoadRejection>,
    /// One read-only session-snapshot document per reached checkpoint, in
    /// ascending checkpoint order; empty unless the executor collects
    /// snapshots (docs/session-snapshot.md, "Non-perturbation proof over the
    /// IRP"). Collecting never perturbs the other
    /// fields: the oracle never reads them.
    pub snapshots: Vec<CaseSnapshotDocument>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// One canonical session-snapshot document an executor collected at one
/// checkpoint of one case (docs/session-snapshot.md, "Non-perturbation
/// proof over the IRP").
pub struct CaseSnapshotDocument {
    /// The checkpoint ordinal the document was taken at.
    pub checkpoint: u32,
    /// Canonical `scxml.session-snapshot/v2` document bytes.
    pub document: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Closed reason the executor's host rejected a document at load (SCXML
/// 1.0 section 5.8), mirroring the machine component's `load-script`
/// failure codes.
pub enum LoadRejection {
    /// The script loader could not fetch an external `<script src>`
    /// (`unavailable`).
    ScriptUnavailable,
    /// A fetched external script failed authentication: URI, media type,
    /// byte bound, pin, or text (`invalid-input` or `payload-too-large`).
    ScriptInvalid,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Ordered observations of every session in one run.
///
/// Records appear in the exact order the executor's host observed them
/// across the root session and every invoked session. The trace is a
/// passive observation: recording never changes execution, and the
/// official `Outcome` oracle never reads it. The host records every
/// observation; it imposes no bound of its own.
pub struct ExecutionTrace {
    /// Ordered observations.
    pub records: Vec<TraceRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// One observation attributed to the session that produced it.
pub struct TraceRecord {
    /// Producing session identifier (`conformance` for the root session,
    /// `conformance.<n>` for the n-th session started anywhere in the run,
    /// at any invoke depth).
    pub session: String,
    /// Invoking session identifier; `None` only for the root session.
    pub parent_session: Option<String>,
    /// What the session did.
    pub observation: TraceObservation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Closed set of observations recorded by the executor's host.
pub enum TraceObservation {
    /// One admitted engine command (start, external event, or resume)
    /// completed and produced its result.
    Command,
    /// One `<raise>` effect placed the named event on the session's
    /// internal queue.
    Raise {
        /// Raised event name.
        event: String,
    },
    /// The engine dequeued the named event and processed it by transition
    /// selection (a transition was selected, or the event was ignored);
    /// recorded after the processing command's [`Self::Command`].
    Processed {
        /// Processed event name.
        event: String,
    },
    /// One `<log>` effect emitted this evaluated record.
    Log(LogRecord),
}

/// Observations of one harness-as-external-client run: the receiving
/// session's execution and the HTTP response the client received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalClientRun {
    /// The receiving session's execution result.
    pub execution: ExecutionResult,
    /// The response returned to the external client.
    pub response: HttpResponse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// A bounded failure returned by an executor.
pub struct ExecutionError {
    /// Stable machine-readable reason code.
    pub code: &'static str,
    /// Human-readable failure description.
    pub message: String,
    /// Disposition category used by the conformance runner.
    pub category: ExecutionFailureCategory,
    /// Engine operations completed before the failure.
    pub commands: u64,
    /// Deterministic fuel consumed before the failure.
    pub fuel_consumed: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Classification of an execution failure.
pub enum ExecutionFailureCategory {
    /// The executor or one of its required capabilities failed.
    Harness,
    /// Execution exhausted the caller-provided command budget.
    CommandBudget,
    /// Execution encountered behavior outside the supported runtime slice.
    Unsupported,
}

impl ExecutionError {
    /// Creates a failure with no commands or fuel charged; callers that ran
    /// engine work set [`Self::commands`] and [`Self::fuel_consumed`].
    #[must_use]
    pub fn new(
        category: ExecutionFailureCategory,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            category,
            commands: 0,
            fuel_consumed: 0,
        }
    }
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ExecutionError {}

/// Implementation identities an executor binds into every report it
/// produces. The compiler and manual-oracle identities stay runner-owned
/// because the runner, not the executor, compiles and judges every case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutorIdentities {
    /// The implementation that executed the engine semantics.
    pub engine: ConformanceImplementationIdentity,
    /// The execution path's runner identity.
    pub runner: ConformanceImplementationIdentity,
    /// Data-model implementation identity keyed by the exact prepared-suite
    /// data-model name.
    pub data_models: BTreeMap<String, ConformanceImplementationIdentity>,
    /// Identity bound for a suite whose data-model name has no entry in
    /// [`Self::data_models`]; such a suite's cases still execute and fail
    /// closed through the executor's own data-model routing.
    pub unrouted_data_model: ConformanceImplementationIdentity,
}

impl ExecutorIdentities {
    /// Returns the identity bound for a suite declaring `datamodel`.
    #[must_use]
    pub fn data_model(&self, datamodel: &str) -> ConformanceImplementationIdentity {
        self.data_models
            .get(datamodel)
            .unwrap_or(&self.unrouted_data_model)
            .clone()
    }
}

/// One extension namespace an executor binds (SCXML 1.0 section 4.10): the
/// package the runner's canonical compiler verifies and lowers the
/// namespace's elements with, and the identity the report records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundExtension {
    /// The verified package manifest.
    pub manifest: scxml_validator_core::ExtensionPackageManifest,
    /// The schema bytes the manifest's `schema-sha256` names.
    pub schema: scxml_validator_core::SchemaSource,
    /// The report identity: namespace URI, package version, and the
    /// SHA-256 of the manifest, schema, and component bytes.
    pub identity: scxml_reporting::ConformanceExtensionIdentity,
}

/// One execution path for compiled conformance cases.
///
/// See the module documentation for the pre/postconditions every
/// implementation must uphold.
pub trait CaseExecutor {
    /// Identities of the implementation this executor runs.
    fn identities(&self) -> ExecutorIdentities;

    /// Whether `execute` and `execute_external_client` collect one read-only
    /// session-snapshot document per reached checkpoint (docs/session-snapshot.md,
    /// "Non-perturbation proof over the IRP").
    /// Default: no collection.
    fn collects_snapshots(&self) -> bool {
        false
    }

    /// The extension namespaces this executor binds, in namespace-URI order.
    /// The runner compiles with exactly their packages and links their
    /// elements as executable content; a document using any other
    /// extension namespace stays `Unsupported`. Default: none.
    fn extensions(&self) -> Vec<BoundExtension> {
        Vec::new()
    }

    /// Runs `ir` as the root session with the scripted external `events`.
    ///
    /// # Errors
    ///
    /// Returns a typed [`ExecutionError`] for budget exhaustion, unsupported
    /// behavior, or a failed execution capability.
    fn execute(
        &self,
        ir_bytes: &[u8],
        ir: &MachineIr,
        events: &[String],
        max_commands: u64,
        sources: &BTreeMap<String, String>,
    ) -> Result<ExecutionResult, ExecutionError>;

    /// Runs `ir` as the receiving session of a harness-as-external-client
    /// case: the harness POSTs `request(location)` to the session's
    /// `BasicHTTP` location, then the session runs to a stable configuration.
    ///
    /// # Errors
    ///
    /// As [`CaseExecutor::execute`].
    fn execute_external_client(
        &self,
        ir_bytes: &[u8],
        ir: &MachineIr,
        request: &dyn Fn(&str) -> HttpRequest,
        max_commands: u64,
        sources: &BTreeMap<String, String>,
    ) -> Result<ExternalClientRun, ExecutionError>;
}

/// Binds `name`, `version`, and the exact implementation `sources` into one
/// report identity.
pub(crate) fn implementation_identity(
    name: &'static str,
    version: &'static str,
    sources: &[&[u8]],
) -> ConformanceImplementationIdentity {
    let mut digest = Sha256::new();
    digest.update(b"scxml.implementation/v1\0");
    digest.update(name.as_bytes());
    digest.update([0]);
    digest.update(version.as_bytes());
    digest.update([0]);
    for source in sources {
        digest.update([0]);
        digest.update(source);
    }
    ConformanceImplementationIdentity {
        name: name.into(),
        version: version.into(),
        source_digest: hex::encode(digest.finalize()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(name: &'static str) -> ConformanceImplementationIdentity {
        implementation_identity(name, "0.1.0", &[name.as_bytes()])
    }

    #[test]
    fn data_model_identities_route_by_exact_name_and_fall_back_explicitly() {
        let identities = ExecutorIdentities {
            engine: identity("engine"),
            runner: identity("runner"),
            data_models: [("ecmascript".to_owned(), identity("ecmascript"))]
                .into_iter()
                .collect(),
            unrouted_data_model: identity("unrouted"),
        };
        assert_eq!(identities.data_model("ecmascript").name, "ecmascript");
        assert_eq!(
            identities.data_model("ECMAScript").name,
            "unrouted",
            "data-model routing uses the exact name"
        );
    }

    #[test]
    fn implementation_identity_binds_name_version_and_every_source() {
        let base = implementation_identity("name", "1", &[b"a", b"b"]);
        assert_eq!(base, implementation_identity("name", "1", &[b"a", b"b"]));
        for other in [
            implementation_identity("other", "1", &[b"a", b"b"]),
            implementation_identity("name", "2", &[b"a", b"b"]),
            implementation_identity("name", "1", &[b"ab"]),
            implementation_identity("name", "1", &[b"a"]),
        ] {
            assert_ne!(base.source_digest, other.source_digest);
        }
    }
}
