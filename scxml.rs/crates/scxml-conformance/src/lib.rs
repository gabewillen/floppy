//! Official-W3C-IRP-first SCXML conformance preparation and execution.
//!
//! Native preparation ingests the W3C `assertions/assert/test/start/dep`
//! manifest, acquires or consumes explicit corpus bytes, invokes a configured
//! XSLT processor, and writes immutable digest-keyed artifacts. The
//! deterministic runner consumes only prepared artifacts, compiles each case
//! through the canonical compiler, and executes the lowered Machine IR only
//! through a [`CaseExecutor`]: the shipped Wasm components under the Wasmtime
//! session host (`WasmExecutor`, behind the `wasmtime` feature).

#![forbid(unsafe_code)]

/// Comparison of a fresh run with a checked-in canonical baseline.
pub mod baseline;
/// Detects prepared cases that require unsupported runtime capabilities.
pub mod capability;
/// Checked-in, digest-bound corrections of upstream W3C IRP defects.
pub mod errata;
/// Execution seam and the observations an executor returns.
pub mod executor;
/// Harness-as-external-client oracles for W3C manual `BasicHTTP` cases.
pub mod external_client;
/// Selects conformance cases by stable manifest metadata.
pub mod filter;
// The harness's outbound network for extension components (documented by
// its own inner docs, whose links resolve in its scope).
#[cfg(feature = "wasmtime")]
pub mod example_server;
/// Defines, validates, reads, and writes prepared corpus manifests.
pub mod manifest;
/// Documented automated equivalents of the W3C IRP manual-case instructions.
pub mod manual_oracle;
/// Evaluates execution results through provenance-specific conformance oracles.
pub mod oracle;
/// Acquires and transforms official corpus inputs into prepared artifacts.
pub mod prepare;
/// Defines canonical run reports and baseline comparisons.
pub mod report;
/// Compiles and runs selected prepared conformance cases.
pub mod runner;
/// The component execution path: shipped Wasm components under Wasmtime.
#[cfg(feature = "wasmtime")]
pub mod wasm;

/// Every repository namespace's own implementation report plan
/// (`namespaces/<name>/irp`, a [`CorpusKind::NamespaceIrp`] corpus) and the
/// extension package manifests (workspace-relative paths) its runs bind, in
/// name order. `env` exercises its slots through the example extension's
/// `<example:fetch>`; `event` binds nothing.
pub const NAMESPACE_SUITES: [(&str, &[&str]); 2] = [
    (
        "env",
        &["components/scxml-extension-example/package/example.extension.json"],
    ),
    ("event", &[]),
];

pub use baseline::{BaselineError, BaselineMismatch, compare_to_baseline};
pub use errata::{
    ERRATA_VERSION, ErrataError, ErrataFile, Erratum, LoadedErrata, NotApplicableCase, Replacement,
    apply_replacements, load_errata, validate_errata,
};
pub use executor::{
    BoundExtension, CaseExecutor, CaseSnapshotDocument, ExecutionError, ExecutionFailureCategory,
    ExecutionResult, ExecutionTrace, ExecutorIdentities, ExternalClientRun, LoadRejection,
    LogRecord, TraceObservation, TraceRecord,
};
pub use filter::{Filters, IdRange};
pub use manifest::{
    CORPUS_FILE, CaseErratum, CaseManifest, ConformanceLevel, CorpusKind, ErrataIdentity,
    ErratumStage, ExpectedOutcome, IrpAssertion, IrpCase, IrpManifest, IrpTest, LoadedCase,
    LoadedSuite, LogOutcome, MANIFEST_VERSION, ManifestError, NotApplicable, PreparationFailure,
    SuiteIdentity, SuiteManifest, TransformerIdentity, flatten_irp_manifest, load_suite,
    parse_irp_manifest, sha256_bytes, sha256_file, source_set_digest, suite_identity_digest,
};
pub use prepare::{
    CorpusTransformer, OfflineCapability, PrepareError, PrepareOptions, PrepareSummary,
    SaxonTransformer, prepare_corpus,
};
pub use report::{
    CaseResult, Comparison, Counts, DiagnosticRecord, Disposition, IdentityPolicy, OracleKind,
    ReportError, RunReport, Stage, Summary, TerminalStatus, compare_reports,
    report_identity_digest,
};
pub use runner::{
    RunOptions, RunnerError, SnapshotSink, run_corpus, run_suite, run_suite_with_snapshot_sink,
};
#[cfg(feature = "wasmtime")]
pub use wasm::{ExtensionRelease, WasmExecutor, WasmExecutorError};
