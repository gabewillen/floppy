//! Versioned canonical diagnostic artifacts and deterministic reporters.
//!
//! Canonical validator and compiler artifacts intentionally preserve the complete
//! source name and UTF-8 source text so independently deployed reporters can
//! render precise snippets without rerunning producer logic. These bytes can therefore contain
//! secrets, personal data, expressions, and local paths from the input. Callers
//! must treat them with the same privacy and retention policy as the source.
//!
//! # Example
//!
//! A producer builds and encodes a validated artifact; an independently
//! deployed reporter decodes the exact bytes, which fail closed on any
//! tampering, unknown field, or unsupported version:
//!
//! ```
//! use scxml_reporting::{
//!     ArtifactEnvelope, ByteSpan, Diagnostic, DiagnosticCategory, Label, Severity,
//!     decode_canonical, encode_canonical,
//! };
//!
//! let source = "<scxml/>";
//! let diagnostic = Diagnostic {
//!     severity: Severity::Error,
//!     code: "SCXML001".into(),
//!     category: DiagnosticCategory::Semantic,
//!     message: "missing version".into(),
//!     primary: Label { span: ByteSpan { start: 1, end: 6 }, message: None },
//!     secondary: Vec::new(),
//!     notes: Vec::new(),
//!     help: Some("add version=\"1.0\"".into()),
//!     spec_references: Vec::new(),
//!     fixes: Vec::new(),
//! };
//! let artifact = ArtifactEnvelope::validator("machine.scxml", source, vec![diagnostic])?;
//! let bytes = encode_canonical(&artifact)?;
//! assert_eq!(decode_canonical(&bytes)?, artifact);
//!
//! // A span outside the preserved source is rejected before encoding.
//! let mut tampered = artifact.clone();
//! if let scxml_reporting::ArtifactPayload::Validator { validator } = &mut tampered.payload {
//!     validator.diagnostics[0].primary.span.end = 99;
//! }
//! assert!(encode_canonical(&tampered).is_err());
//! # Ok::<(), scxml_reporting::ArtifactError>(())
//! ```

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::io;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_types::wasm_bound;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use unicode_width::UnicodeWidthChar as _;

pub mod explain;
mod snippet;

/// Current canonical artifact envelope version.
pub const ARTIFACT_VERSION: &str = "scxml.report/v3";
/// Validator payload discriminator accepted by the reporters in this release.
pub const VALIDATOR_KIND: &str = "validator";
/// Compiler payload discriminator.
pub const COMPILER_KIND: &str = "compiler";
/// Conformance payload discriminator.
pub const CONFORMANCE_KIND: &str = "conformance";
/// Conformance report payload version accepted by this reporter release.
pub const CONFORMANCE_REPORT_VERSION: u32 = 9;
/// Machine IR schema version emitted by the compiler and accepted in compiler
/// package recipes. `scxml-compiler-core` re-exports this constant as the
/// single source of truth for the IR format version.
pub const MACHINE_IR_VERSION: u32 = 5;
/// Canonical WIT interface through which a machine component obtains
/// Machine IR bytes of [`MACHINE_IR_VERSION`] (runtime mode: host-provided;
/// packaged mode: a composed data-only provider).
pub const MACHINE_IR_SOURCE_INTERFACE: &str = "stateforward:scxml/ir-source@0.2.0";
/// Generic machine template world recorded in compiler package recipes.
pub const MACHINE_TEMPLATE_WORLD: &str = "stateforward:scxml/machine@0.2.0";
/// Generic machine template version recorded in compiler package recipes.
pub const MACHINE_TEMPLATE_VERSION: &str = "0.2.0";
/// Media type of the `lowered-ir` compiler phase (portable data, not Wasm).
pub const LOWERED_IR_MEDIA_TYPE: &str = "application/vnd.stateforward.scxml-machine-ir+json";
/// File extension of the `lowered-ir` compiler phase.
pub const LOWERED_IR_FILE_EXTENSION: &str = "scxml-ir.json";
/// Media type of the `component-wasm` compiler phase.
pub const COMPONENT_WASM_MEDIA_TYPE: &str = "application/wasm";
/// File extension of the `component-wasm` compiler phase.
pub const COMPONENT_WASM_FILE_EXTENSION: &str = "component.wasm";
/// SARIF schema emitted by [`render_sarif`].
pub const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
/// Maximum accepted canonical envelope size (32 MiB).
pub const MAX_CANONICAL_BYTES: usize = 32 * 1024 * 1024;
/// Maximum rendered report size (32 MiB), on every build: rendered output
/// can grow faster than its input (each diagnostic repeats a source excerpt),
/// so this is an amplification guard rather than a resource bound.
///
/// Rendering fails closed at this boundary instead of truncating diagnostics or
/// allowing repeated source excerpts to amplify a bounded artifact without a
/// corresponding output bound.
pub const MAX_RENDERED_BYTES: usize = 32 * 1024 * 1024;
/// Maximum projected bytes copied or allocated while constructing one SARIF
/// value tree, on every build (an amplification guard, like
/// [`MAX_RENDERED_BYTES`]).
///
/// The renderer accounts for repeated SARIF projections before cloning any
/// attacker-controlled strings. This bounds construction independently from
/// the serialized output limit.
pub const MAX_SARIF_CONSTRUCTION_BYTES: usize = 32 * 1024 * 1024;
/// Maximum preserved SCXML source size (16 MiB) (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_SOURCE_BYTES: usize = wasm_bound(16 * 1024 * 1024);
/// Maximum diagnostics in one source-backed payload (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_DIAGNOSTICS: usize = wasm_bound(100_000);
/// Maximum selected conformance cases in one payload.
pub const MAX_CONFORMANCE_CASES: usize = 1_000_000;
/// Maximum UTF-8 byte length of one canonical conformance identity string.
pub const MAX_CONFORMANCE_IDENTITY_STRING_BYTES: usize = 64 * 1024;
/// Maximum entries in one canonical conformance identity collection.
pub const MAX_CONFORMANCE_IDENTITY_ITEMS: usize = 100_000;
/// Maximum canonical compiler package recipe size (1 MiB) (wasm builds only; unbounded natively, see [`scxml_types::wasm_bound`]).
pub const MAX_PACKAGE_RECIPE_BYTES: usize = wasm_bound(1024 * 1024);

/// Diagnostic severity ordered from least to most severe.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// Supplemental information.
    Note,
    /// Recoverable concern.
    Warning,
    /// Validation failure.
    Error,
}

impl Severity {
    /// Returns the stable lowercase spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Stable diagnostic category for filtering and AI repair planning.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticCategory {
    /// XML or source parsing.
    Syntax,
    /// XSD structural validation or schema setup.
    Schema,
    /// Normative SCXML semantic constraint.
    Semantic,
    /// Non-fatal quality or liveness lint.
    Lint,
    /// Extension package metadata validation.
    Package,
    /// Other bounded validator failure.
    Internal,
}

/// Confidence level for applying a structured edit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Applicability {
    /// The edit can be applied without changing intended semantics.
    MachineApplicable,
    /// The edit is plausible but requires semantic review.
    MaybeIncorrect,
    /// The edit contains placeholder text (such as `…`) that must be replaced
    /// by a real value before the result is valid.
    HasPlaceholders,
    /// No applicability guarantee is made.
    Unspecified,
}

/// Half-open UTF-8 byte range in the preserved source.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ByteSpan {
    /// Inclusive UTF-8 byte offset.
    pub start: u64,
    /// Exclusive UTF-8 byte offset.
    pub end: u64,
}

/// One source annotation attached to a diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    /// Labeled byte range.
    pub span: ByteSpan,
    /// Explanation of this range when available.
    pub message: Option<String>,
}

/// One replacement edit over the preserved source.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TextEdit {
    /// Byte range replaced by [`Self::replacement`].
    pub span: ByteSpan,
    /// Exact UTF-8 replacement text.
    pub replacement: String,
}

/// One clippy-style suggested repair.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fix {
    /// Human-readable repair summary.
    pub message: String,
    /// Confidence level for applying the edits.
    pub applicability: Applicability,
    /// One or more non-overlapping source edits in ascending order.
    pub edits: Vec<TextEdit>,
}

/// Complete machine-oriented validator diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    /// Severity class.
    pub severity: Severity,
    /// Stable rule or backend code.
    pub code: String,
    /// Stable category independent of presentation.
    pub category: DiagnosticCategory,
    /// Primary summary.
    pub message: String,
    /// Primary source annotation.
    pub primary: Label,
    /// Related source annotations.
    pub secondary: Vec<Label>,
    /// Supplemental notes.
    pub notes: Vec<String>,
    /// Actionable human and AI repair guidance.
    pub help: Option<String>,
    /// Normative or explanatory specification references.
    pub spec_references: Vec<String>,
    /// Structured repairs where the validator can state them honestly.
    pub fixes: Vec<Fix>,
}

/// Authoritative aggregate counts stored with the validator payload.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    /// Number of error diagnostics.
    pub errors: u64,
    /// Number of warning diagnostics.
    pub warnings: u64,
    /// Number of note diagnostics.
    pub notes: u64,
    /// Total diagnostics.
    pub total: u64,
}

impl Counts {
    /// Computes authoritative counts for a diagnostic sequence.
    #[must_use]
    pub fn from_diagnostics(diagnostics: &[Diagnostic]) -> Self {
        let mut result = Self::default();
        for diagnostic in diagnostics {
            match diagnostic.severity {
                Severity::Note => result.notes += 1,
                Severity::Warning => result.warnings += 1,
                Severity::Error => result.errors += 1,
            }
            result.total += 1;
        }
        result
    }
}

/// Validator-specific canonical payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatorArtifact {
    /// Display name or path supplied by the caller.
    pub source_name: String,
    /// Complete original UTF-8 source. This is privacy-sensitive data.
    pub source_text: String,
    /// Deterministically ordered complete diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// Authoritative aggregate counts checked during decoding.
    pub counts: Counts,
}

/// Authoritative compiler outcome.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompilerStatus {
    /// Validation or lowering rejected the source.
    Rejected,
    /// The producer emitted the described artifact.
    Emitted,
}

/// Compiler-produced artifact metadata. Reporters never recreate these decisions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerOutput {
    /// Honest artifact phase such as `lowered-ir`.
    pub artifact_kind: String,
    /// Media type selected by the compiler.
    pub media_type: String,
    /// Recommended file extension.
    pub file_extension: String,
    /// Exact emitted byte length.
    pub byte_length: u64,
    /// SHA-256 of the exact emitted bytes.
    pub sha256: String,
    /// Canonical compact JSON for the compiler-owned package recipe.
    pub package_recipe: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum CompilerArtifactPhase {
    LoweredIr,
    CoreWasm,
    ComponentWasm,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerPackageRecipe {
    template_world: String,
    template_version: String,
    template_sha256: Option<String>,
    ir_sha256: String,
    ir_source_interface: String,
    linked_extension_components: Vec<String>,
    next_phase: Option<CompilerArtifactPhase>,
}

/// Canonical compiler report payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerArtifact {
    /// Display name or path supplied by the caller.
    pub source_name: String,
    /// Complete original UTF-8 source.
    pub source_text: String,
    /// Authoritative compiler diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// Authoritative aggregate counts.
    pub counts: Counts,
    /// Producer-selected terminal outcome.
    pub status: CompilerStatus,
    /// Producer-selected output metadata, present only for `emitted`.
    pub output: Option<CompilerOutput>,
}

/// IRP conformance level carried by each selected case.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConformanceLevel {
    /// Normative mandatory assertion.
    Mandatory,
    /// Optional assertion.
    Optional,
}

/// Stable reason code carried by every [`ConformanceDisposition::NotApplicable`] case.
pub const NOT_APPLICABLE_REASON_CODE: &str = "w3c-not-applicable";

/// Authoritative IRP case disposition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConformanceDisposition {
    /// The case passed its oracle.
    Passed,
    /// The case ran and failed its oracle.
    Failed,
    /// Required behavior is not implemented.
    Unsupported,
    /// The prepared case or source is invalid.
    Invalid,
    /// The harness failed independently of the case result.
    HarnessError,
    /// The case was selected but not applicable under the recorded filters.
    Skipped,
    /// The W3C declares the case not applicable to the prepared data model
    /// (named by the suite errata identity); outside the applicable
    /// denominator and never executed.
    NotApplicable,
}

/// Furthest producer pipeline stage reached by one conformance case.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConformanceStage {
    /// Case selection.
    Selection,
    /// Corpus acquisition or transformation.
    Preparation,
    /// Prepared-artifact integrity verification.
    Integrity,
    /// SCXML parsing.
    Parse,
    /// Structural or semantic validation.
    Validation,
    /// Lowering to deterministic machine IR.
    Lowering,
    /// Executable capability detection.
    Capability,
    /// Engine execution.
    Execution,
    /// Expected-outcome evaluation.
    Oracle,
    /// Successful completion of all stages.
    Complete,
}

impl ConformanceStage {
    /// Returns the stable lowercase spelling used by text reporters.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Selection => "selection",
            Self::Preparation => "preparation",
            Self::Integrity => "integrity",
            Self::Parse => "parse",
            Self::Validation => "validation",
            Self::Lowering => "lowering",
            Self::Capability => "capability",
            Self::Execution => "execution",
            Self::Oracle => "oracle",
            Self::Complete => "complete",
        }
    }
}

/// Oracle that decided a conformance case whose execution reached oracle
/// evaluation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConformanceOracleKind {
    /// Official W3C protocol: exactly one `Outcome` log whose value is `pass`,
    /// in a root session finished in exactly the `pass` final state.
    OutcomeLog,
    /// Documented automated equivalent of a W3C manual instruction; valid
    /// only for manual cases of an official corpus.
    ManualEquivalent,
    /// Non-official synthetic-harness expectation.
    SyntheticExpectation,
    /// A namespace suite's rejection oracle: the canonical compiler rejects
    /// the document at parse or validation with exactly the error codes
    /// the suite manifest lists, and nothing runs. Valid only for
    /// namespace corpora.
    DocumentRejection,
}

impl ConformanceOracleKind {
    /// Returns the stable lowercase spelling used by text reporters.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutcomeLog => "outcome-log",
            Self::ManualEquivalent => "manual-equivalent",
            Self::SyntheticExpectation => "synthetic-expectation",
            Self::DocumentRejection => "document-rejection",
        }
    }
}

/// Observable terminal status reported after execution began.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConformanceTerminalStatus {
    /// The machine is stable and may accept more events.
    Running,
    /// The machine reached its top-level final state.
    Finished,
    /// The processor rejected the document at load (for example an
    /// external `<script src>` that could not be fetched, SCXML 1.0 section
    /// 5.8): no session ran, so no configuration, command, or fuel exists.
    Rejected,
}

/// Provenance class for the corpus behind a canonical conformance run.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConformanceCorpusKind {
    /// Corpus prepared from the official W3C implementation report plan.
    OfficialIrp,
    /// Synthetic fixtures that exercise the harness without claiming IRP passes.
    SyntheticHarness,
    /// A repository namespace's own implementation report plan
    /// (`namespaces/<name>/irp`): W3C IRP-format assertions over the
    /// namespace's README, prepared by the W3C templates; its passes are
    /// never W3C IRP passes.
    NamespaceIrp,
}

/// Exact transformer identity captured while preparing a corpus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceTransformerIdentity {
    /// Stable transformer implementation name.
    pub name: String,
    /// Captured transformer version output.
    pub version: String,
    /// Exact configured command argument vector, excluding per-case paths.
    pub command_argv: Vec<String>,
    /// SHA-256 of the bounded command/artifact identity when resolution was available.
    pub executable_digest: Option<String>,
}

/// Immutable prepared-corpus identity required to substantiate a run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceSuiteIdentity {
    /// Stable suite identifier.
    pub id: String,
    /// Suite release or snapshot version.
    pub version: String,
    /// Official or synthetic corpus provenance class.
    pub kind: ConformanceCorpusKind,
    /// Exact manifest source location or acquisition identifier.
    pub manifest_source: String,
    /// Optional pinned source revision.
    pub revision: Option<String>,
    /// SHA-256 of the complete prepared corpus identity.
    pub corpus_digest: String,
    /// SHA-256 of the exact acquired source set.
    pub source_set_digest: String,
    /// SHA-256 of the source manifest bytes.
    pub manifest_digest: String,
    /// Canonically ordered template identity to SHA-256 mapping.
    pub template_digests: BTreeMap<String, String>,
    /// Transformer identity when corpus specialization required one.
    pub transformer: Option<ConformanceTransformerIdentity>,
    /// Data model selected during corpus specialization.
    pub datamodel: String,
    /// Optional licensing or acquisition provenance statement.
    pub provenance: Option<String>,
    /// Checked-in IRP errata applied during preparation, when any.
    pub errata: Option<ConformanceErrataIdentity>,
}

/// Identity of the checked-in IRP errata applied while preparing a corpus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceErrataIdentity {
    /// Portable errata file name.
    pub source: String,
    /// SHA-256 of the exact errata file bytes.
    pub digest: String,
    /// Corrected case id to erratum id.
    pub applied: BTreeMap<String, String>,
    /// Case ids the W3C declares not applicable, in canonical case order.
    pub not_applicable: Vec<String>,
}

/// Inclusive numeric case-ID range in canonical run filters.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceIdRange {
    /// Lowest selected numeric case identifier.
    pub start: u64,
    /// Highest selected numeric case identifier.
    pub end: u64,
}

/// Complete normalized selectors used to produce a conformance report.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceFilters {
    /// Exact case identifiers included by the run.
    pub ids: Vec<String>,
    /// Inclusive numeric case-ID ranges included by the run.
    pub id_ranges: Vec<ConformanceIdRange>,
    /// Specification fragment identifiers included by the run.
    pub spec_ids: Vec<String>,
    /// Specification section numbers included by the run.
    pub spec_nums: Vec<String>,
    /// Conformance levels included by the run.
    pub conformance: Vec<ConformanceLevel>,
    /// Data-model identifiers included by the run.
    pub datamodels: Vec<String>,
    /// Tags required by the run.
    pub tags: Vec<String>,
}

/// Closed schema-validation policy used by the canonical compiler.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConformanceSchemaPolicy {
    /// XML and semantic validation only.
    Off,
    /// Validate available schemas while allowing missing schemas.
    Lax,
    /// Require schemas for every used namespace.
    Strict,
}

/// Required immutable identity of one implementation participating in a run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceImplementationIdentity {
    /// Closed producer-selected implementation name.
    pub name: String,
    /// Implementation package or contract version.
    pub version: String,
    /// Lowercase SHA-256 over the exact implementation sources used by the producer.
    pub source_digest: String,
}

/// Identity of one extension package a conformance run bound: what its
/// namespace's elements were compiled against and executed by.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceExtensionIdentity {
    /// The exact namespace URI the package binds.
    pub namespace_uri: String,
    /// The package version its manifest declares.
    pub package_version: String,
    /// Lowercase SHA-256 of the exact package manifest bytes.
    pub manifest_sha256: String,
    /// Lowercase SHA-256 of the schema bytes (the manifest's `schema-sha256`).
    pub schema_sha256: String,
    /// Lowercase SHA-256 of the extension component bytes.
    pub component_sha256: String,
}

/// Complete producer-owned identity for one selected conformance run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceRunIdentity {
    /// Exact immutable prepared-suite identity.
    pub suite: ConformanceSuiteIdentity,
    /// Schema-validation policy used by the compiler.
    pub schema_policy: ConformanceSchemaPolicy,
    /// Immutable canonical compiler implementation identity.
    pub compiler: ConformanceImplementationIdentity,
    /// Immutable deterministic engine implementation identity.
    pub engine: ConformanceImplementationIdentity,
    /// Immutable data-model adapter implementation identity.
    pub data_model: ConformanceImplementationIdentity,
    /// Immutable conformance runner implementation identity.
    pub runner: ConformanceImplementationIdentity,
    /// Immutable manual-equivalent oracle table identity: name, table
    /// version, and source digest.
    pub manual_oracle: ConformanceImplementationIdentity,
    /// The extension packages the run bound, sorted by namespace URI;
    /// empty when the run bound none.
    pub extensions: Vec<ConformanceExtensionIdentity>,
    /// Machine IR schema version used by the run.
    pub ir_version: u32,
    /// Complete normalized run filters.
    pub filters: ConformanceFilters,
    /// Optional supplemental source-control revision supplied by the caller.
    pub run_revision: Option<String>,
    /// SHA-256 binding over producer identity, filters, summary, and case results.
    pub report_digest: String,
}

/// One producer-authored conformance diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceDiagnostic {
    /// Stable producer code.
    pub code: String,
    /// Stable producer severity spelling.
    pub severity: Severity,
    /// Producer-authored message.
    pub message: String,
}

/// One selected IRP case and its authoritative producer disposition.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceCase {
    /// Stable case identifier.
    pub id: String,
    /// Stable assertion identifier.
    pub assertion_id: String,
    /// Human-readable specification section number.
    pub spec_num: String,
    /// Specification anchor.
    pub spec_id: String,
    /// Selected data model.
    pub datamodel: String,
    /// Mandatory or optional classification.
    pub conformance: ConformanceLevel,
    /// Whether the source manifest classifies this case as manual.
    pub manual: bool,
    /// Producer-authored terminal stage.
    pub stage: ConformanceStage,
    /// Producer-authored terminal disposition.
    pub disposition: ConformanceDisposition,
    /// Stable reason code when supplied.
    pub reason_code: Option<String>,
    /// Human-readable reason when supplied.
    pub reason: Option<String>,
    /// Producer diagnostics for the case.
    pub diagnostics: Vec<ConformanceDiagnostic>,
    /// Final active configuration reported by the engine.
    pub configuration: Vec<String>,
    /// Terminal engine status when execution produced one.
    pub terminal_status: Option<ConformanceTerminalStatus>,
    /// Engine commands charged to the case.
    pub commands: u64,
    /// Consumed deterministic fuel.
    pub fuel_consumed: u64,
    /// Oracle that decided the case; present exactly for `Passed` at
    /// `Complete` and `Failed` at `Oracle`.
    pub oracle: Option<ConformanceOracleKind>,
    /// Documented erratum that corrected the executed official artifact;
    /// equals the suite errata identity's entry for this case.
    pub erratum: Option<String>,
}

/// Authoritative conformance aggregate counts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceCounts {
    /// Applicable selected cases.
    pub applicable_selected: u64,
    /// Passed cases.
    pub passed: u64,
    /// Failed cases.
    pub failed: u64,
    /// Unsupported cases.
    pub unsupported: u64,
    /// Invalid cases.
    pub invalid: u64,
    /// Harness-error cases.
    pub harness_error: u64,
    /// Skipped cases.
    pub skipped: u64,
    /// Not-applicable cases (outside the applicable denominator).
    pub not_applicable: u64,
    /// Failed plus unsupported plus invalid plus harness-error.
    pub not_passing: u64,
    /// Passed cases executed from an erratum-corrected artifact (a subset
    /// of `passed`).
    pub passed_under_erratum: u64,
}

/// Producer-authored conformance summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceSummary {
    /// All selected cases, including skipped cases.
    pub total_selected: u64,
    /// Top-level counts.
    pub counts: ConformanceCounts,
    /// Mandatory counts.
    pub mandatory: ConformanceCounts,
    /// Optional counts.
    pub optional: ConformanceCounts,
    /// Counts grouped by specification anchor.
    pub by_spec: BTreeMap<String, ConformanceCounts>,
    /// Counts grouped by data model.
    pub by_datamodel: BTreeMap<String, ConformanceCounts>,
}

/// Canonical official-IRP conformance report payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceArtifact {
    /// Report schema version owned by the runner.
    pub report_version: u32,
    /// Complete suite, filter, toolchain, and run revision identity.
    pub identity: ConformanceRunIdentity,
    /// Authoritative producer summary.
    pub summary: ConformanceSummary,
    /// Complete deterministically ordered selected cases.
    pub cases: Vec<ConformanceCase>,
}

/// Payload variants supported by `scxml.report/v3`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ArtifactPayload {
    /// Parser/XSD/semantic validator result.
    Validator {
        /// Validator-specific payload.
        validator: ValidatorArtifact,
    },
    /// Compiler validation/lowering result.
    Compiler {
        /// Compiler-specific payload.
        compiler: CompilerArtifact,
    },
    /// Official-IRP conformance run result.
    Conformance {
        /// Conformance-specific payload.
        conformance: Box<ConformanceArtifact>,
    },
}

/// Versioned independently deployable artifact envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactEnvelope {
    /// Exact artifact contract version.
    pub version: String,
    /// Exactly one kind-tagged payload.
    #[serde(flatten)]
    pub payload: ArtifactPayload,
}

impl ArtifactEnvelope {
    /// Builds the current validator artifact and verifies all invariants.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError`] when a diagnostic or fix span is invalid or a
    /// structured edit sequence overlaps.
    pub fn validator(
        source_name: impl Into<String>,
        source_text: impl Into<String>,
        mut diagnostics: Vec<Diagnostic>,
    ) -> Result<Self, ArtifactError> {
        normalize_diagnostics(&mut diagnostics);
        let artifact = Self {
            version: ARTIFACT_VERSION.into(),
            payload: ArtifactPayload::Validator {
                validator: ValidatorArtifact {
                    source_name: source_name.into(),
                    source_text: source_text.into(),
                    counts: Counts::from_diagnostics(&diagnostics),
                    diagnostics,
                },
            },
        };
        artifact.check().map(|()| artifact)
    }

    /// Builds a canonical compiler payload from producer-owned decisions.
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError`] when diagnostics, status, output metadata, or
    /// package-recipe bytes violate the canonical contract.
    pub fn compiler(
        source_name: impl Into<String>,
        source_text: impl Into<String>,
        mut diagnostics: Vec<Diagnostic>,
        status: CompilerStatus,
        output: Option<CompilerOutput>,
    ) -> Result<Self, ArtifactError> {
        normalize_diagnostics(&mut diagnostics);
        let artifact = Self {
            version: ARTIFACT_VERSION.into(),
            payload: ArtifactPayload::Compiler {
                compiler: CompilerArtifact {
                    source_name: source_name.into(),
                    source_text: source_text.into(),
                    counts: Counts::from_diagnostics(&diagnostics),
                    diagnostics,
                    status,
                    output,
                },
            },
        };
        artifact.check().map(|()| artifact)
    }

    /// Builds a canonical conformance payload from a complete producer report.
    ///
    /// Set-like filter collections are normalized, producer case order and
    /// observations are preserved, and `identity.report_digest` is replaced
    /// with the digest computed by [`conformance_report_digest`].
    ///
    /// # Errors
    ///
    /// Returns [`ArtifactError`] when ordering or aggregate invariants fail.
    pub fn conformance(mut conformance: ConformanceArtifact) -> Result<Self, ArtifactError> {
        normalize_conformance_identity(&mut conformance.identity);
        conformance.identity.report_digest = conformance_report_digest(&conformance)?;
        let artifact = Self {
            version: ARTIFACT_VERSION.into(),
            payload: ArtifactPayload::Conformance {
                conformance: Box::new(conformance),
            },
        };
        artifact.check().map(|()| artifact)
    }

    /// Returns the stable payload discriminator.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match &self.payload {
            ArtifactPayload::Validator { .. } => VALIDATOR_KIND,
            ArtifactPayload::Compiler { .. } => COMPILER_KIND,
            ArtifactPayload::Conformance { .. } => CONFORMANCE_KIND,
        }
    }

    /// Returns the validator payload when this is a validator artifact.
    #[must_use]
    pub fn validator_payload(&self) -> Option<&ValidatorArtifact> {
        match &self.payload {
            ArtifactPayload::Validator { validator } => Some(validator),
            _ => None,
        }
    }

    /// Returns the compiler payload when this is a compiler artifact.
    #[must_use]
    pub fn compiler_payload(&self) -> Option<&CompilerArtifact> {
        match &self.payload {
            ArtifactPayload::Compiler { compiler } => Some(compiler),
            _ => None,
        }
    }

    /// Returns the conformance payload when this is a conformance artifact.
    #[must_use]
    pub fn conformance_payload(&self) -> Option<&ConformanceArtifact> {
        match &self.payload {
            ArtifactPayload::Conformance { conformance } => Some(conformance.as_ref()),
            _ => None,
        }
    }

    /// Returns mutable validator payload access for producer-side assembly.
    #[cfg(test)]
    fn validator_payload_mut(&mut self) -> Option<&mut ValidatorArtifact> {
        match &mut self.payload {
            ArtifactPayload::Validator { validator } => Some(validator),
            _ => None,
        }
    }

    /// Validates version, kind, counts, UTF-8 byte boundaries, spans, and fixes.
    ///
    /// # Errors
    ///
    /// Returns a stable validation error for malformed or tampered artifacts.
    pub fn validate(&self) -> Result<(), ArtifactError> {
        let span = observe(Module::Reporting, Operation::ReportingArtifactValidate);
        let result = self.check();
        if result.is_err() {
            span.fail(ErrorCategory::Validation);
        }
        result
    }

    /// Canonical invariant check shared by [`Self::validate`] and the
    /// encode, decode, render, and builder boundaries; it runs inside their
    /// span and records no telemetry of its own.
    fn check(&self) -> Result<(), ArtifactError> {
        if self.version != ARTIFACT_VERSION {
            return Err(ArtifactError::UnsupportedVersion(self.version.clone()));
        }
        match &self.payload {
            ArtifactPayload::Validator { validator } => {
                validate_diagnostic_payload(
                    &validator.source_text,
                    &validator.diagnostics,
                    validator.counts,
                )?;
            }
            ArtifactPayload::Compiler { compiler } => {
                validate_diagnostic_payload(
                    &compiler.source_text,
                    &compiler.diagnostics,
                    compiler.counts,
                )?;
                match (compiler.status, &compiler.output) {
                    (CompilerStatus::Rejected, None) => {}
                    (CompilerStatus::Emitted, Some(output)) => {
                        if compiler.counts.errors != 0 {
                            return Err(ArtifactError::InconsistentCompilerStatus);
                        }
                        validate_compiler_output(output)?;
                    }
                    _ => return Err(ArtifactError::InconsistentCompilerStatus),
                }
            }
            ArtifactPayload::Conformance { conformance } => {
                validate_conformance(conformance)?;
            }
        }
        Ok(())
    }
}

/// Canonical artifact decoding or invariant failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArtifactError {
    /// Input is not canonical JSON for the supported shape.
    InvalidJson(String),
    /// Envelope version is not supported.
    UnsupportedVersion(String),
    /// Envelope kind is not supported by this release.
    UnsupportedKind(String),
    /// A kind-specific payload version is not supported by this release.
    UnsupportedPayloadVersion {
        /// Stable payload discriminator.
        kind: &'static str,
        /// Rejected numeric payload version.
        version: u32,
    },
    /// Stored counts do not match the complete diagnostic list.
    InconsistentCounts {
        /// Counts stored in the artifact.
        declared: Counts,
        /// Counts computed from diagnostics.
        actual: Counts,
    },
    /// Compiler status and output metadata contradict each other.
    InconsistentCompilerStatus,
    /// Compiler package-recipe bytes are not canonical compact JSON.
    NonCanonicalPackageRecipe,
    /// Conformance summary does not match its complete case sequence.
    InconsistentConformanceSummary,
    /// Conformance identity is incomplete, malformed, or noncanonical.
    InvalidConformanceIdentity(&'static str),
    /// Stored conformance digest does not bind the exact canonical payload.
    ConformanceDigestMismatch,
    /// A conformance case violates its authoritative cross-field invariants.
    InvalidConformanceCase(&'static str),
    /// A diagnostic code is not a code of a known family ([`is_known_code`]).
    UnknownCode,
    /// A source span is outside the source or not on UTF-8 boundaries.
    InvalidSpan(ByteSpan),
    /// Suggested edits are empty, unsorted, or overlapping.
    OverlappingEdits,
    /// Diagnostics or nested unordered collections are not normalized.
    NonCanonicalOrdering,
    /// JSON bytes do not exactly match the canonical compact encoding.
    NonCanonicalJson,
    /// A bounded artifact collection or byte payload exceeded its limit.
    LimitExceeded(&'static str),
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(message) => {
                write!(formatter, "invalid canonical artifact JSON: {message}")
            }
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported artifact version `{version}`")
            }
            Self::UnsupportedKind(kind) => write!(formatter, "unsupported artifact kind `{kind}`"),
            Self::UnsupportedPayloadVersion { kind, version } => {
                write!(formatter, "unsupported {kind} payload version `{version}`")
            }
            Self::InconsistentCounts { declared, actual } => write!(
                formatter,
                "artifact counts are inconsistent: declared {declared:?}, actual {actual:?}"
            ),
            Self::InconsistentCompilerStatus => formatter
                .write_str("compiler status, diagnostics, and output metadata are inconsistent"),
            Self::NonCanonicalPackageRecipe => {
                formatter.write_str("compiler package recipe is not canonical compact JSON")
            }
            Self::InconsistentConformanceSummary => formatter
                .write_str("conformance summary is inconsistent with the complete case list"),
            Self::InvalidConformanceIdentity(field) => {
                write!(
                    formatter,
                    "invalid canonical conformance identity field `{field}`"
                )
            }
            Self::ConformanceDigestMismatch => formatter
                .write_str("conformance report digest does not match the canonical payload"),
            Self::InvalidConformanceCase(field) => {
                write!(
                    formatter,
                    "invalid canonical conformance case field `{field}`"
                )
            }
            Self::UnknownCode => {
                formatter.write_str("diagnostic code is not a code of a known family")
            }
            Self::InvalidSpan(span) => write!(
                formatter,
                "invalid UTF-8 source span {}..{}",
                span.start, span.end
            ),
            Self::OverlappingEdits => {
                formatter.write_str("fix edits must be non-empty, sorted, and non-overlapping")
            }
            Self::NonCanonicalOrdering => {
                formatter.write_str("artifact collections are not canonically ordered")
            }
            Self::NonCanonicalJson => formatter.write_str("artifact bytes are not canonical JSON"),
            Self::LimitExceeded(limit) => write!(formatter, "artifact exceeds {limit}"),
        }
    }
}

impl std::error::Error for ArtifactError {}

/// Encodes a validated artifact as deterministic compact UTF-8 JSON bytes.
///
/// # Errors
///
/// Returns an invariant or serialization failure. Object field order is defined
/// by the structs and all unordered values must be normalized by the producer.
pub fn encode_canonical(artifact: &ArtifactEnvelope) -> Result<Vec<u8>, ArtifactError> {
    let span = observe(Module::Reporting, Operation::ReportingEncodeCanonical);
    let result = artifact.check().and_then(|()| {
        let bytes = serde_json::to_vec(artifact)
            .map_err(|error| ArtifactError::InvalidJson(error.to_string()))?;
        if bytes.len() > MAX_CANONICAL_BYTES {
            return Err(ArtifactError::LimitExceeded(
                "maximum canonical byte length",
            ));
        }
        Ok(bytes)
    });
    if result.is_err() {
        span.fail(ErrorCategory::Report);
    } else {
        span.event(SignalEvent::ArtifactProduced);
    }
    result
}

/// Decodes and validates supported canonical artifact bytes.
///
/// # Errors
///
/// Rejects unknown fields, unknown versions/kinds, inconsistent counts, invalid
/// spans, invalid UTF-8 boundaries, and overlapping edits.
pub fn decode_canonical(bytes: &[u8]) -> Result<ArtifactEnvelope, ArtifactError> {
    let span = observe(Module::Reporting, Operation::ReportingDecodeCanonical);
    let result = if bytes.len() > MAX_CANONICAL_BYTES {
        Err(ArtifactError::LimitExceeded(
            "maximum canonical byte length",
        ))
    } else {
        serde_json::from_slice::<Value>(bytes)
            .map_err(|error| ArtifactError::InvalidJson(error.to_string()))
            .and_then(|value| {
                let version = value
                    .get("version")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ArtifactError::InvalidJson("missing string `version`".into()))?;
                if version != ARTIFACT_VERSION {
                    return Err(ArtifactError::UnsupportedVersion(version.into()));
                }
                let kind = value
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ArtifactError::InvalidJson("missing string `kind`".into()))?;
                if !matches!(kind, VALIDATOR_KIND | COMPILER_KIND | CONFORMANCE_KIND) {
                    return Err(ArtifactError::UnsupportedKind(kind.into()));
                }
                serde_json::from_value::<ArtifactEnvelope>(value)
                    .map_err(|error| ArtifactError::InvalidJson(error.to_string()))
            })
            .and_then(|artifact| {
                artifact.check()?;
                let canonical = serde_json::to_vec(&artifact)
                    .map_err(|error| ArtifactError::InvalidJson(error.to_string()))?;
                if canonical != bytes {
                    return Err(ArtifactError::NonCanonicalJson);
                }
                Ok(artifact)
            })
    };
    if result.is_err() {
        span.fail(ErrorCategory::Parse);
    } else {
        span.event(SignalEvent::ArtifactAccepted);
    }
    result
}

/// Color behavior for rustc-style text rendering.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColorChoice {
    /// Always emit ANSI color.
    Always,
    /// Never emit ANSI color.
    #[default]
    Never,
}

/// Deterministic rustc-style reporter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RustcRenderer {
    /// ANSI color behavior.
    pub color: ColorChoice,
}

impl RustcRenderer {
    /// Renders one validated canonical artifact without changing producer decisions.
    ///
    /// # Errors
    ///
    /// Returns the canonical validation failure for a malformed envelope or
    /// span, or [`ArtifactError::LimitExceeded`] when complete output would
    /// exceed [`MAX_RENDERED_BYTES`]. No partial report is returned.
    pub fn render(self, artifact: &ArtifactEnvelope) -> Result<String, ArtifactError> {
        let span = observe(Module::Reporting, Operation::ReportingRustcRender);
        let result = (|| {
            artifact.check()?;
            let mut output = RenderBuffer::new(MAX_RENDERED_BYTES);
            match &artifact.payload {
                ArtifactPayload::Validator { validator } => snippet::render_source_diagnostics(
                    &mut output,
                    &validator.source_name,
                    &validator.source_text,
                    &validator.diagnostics,
                    validator.counts,
                    self.color,
                )?,
                ArtifactPayload::Compiler { compiler } => {
                    snippet::render_source_diagnostics(
                        &mut output,
                        &compiler.source_name,
                        &compiler.source_text,
                        &compiler.diagnostics,
                        compiler.counts,
                        self.color,
                    )?;
                    append_format(
                        &mut output,
                        format_args!("compiler status: {:?}\n", compiler.status),
                    )?;
                    if let Some(product) = &compiler.output {
                        append_format(
                            &mut output,
                            format_args!(
                                "compiler output: {} ({} bytes, sha256 {})",
                                TerminalText(&product.artifact_kind),
                                product.byte_length,
                                TerminalText(&product.sha256)
                            ),
                        )?;
                        output.push_char('\n')?;
                    }
                }
                ArtifactPayload::Conformance { conformance } => {
                    render_conformance_text(&mut output, conformance)?;
                }
            }
            Ok(output.finish())
        })();
        if result.is_err() {
            span.fail(ErrorCategory::Report);
        } else {
            span.event(SignalEvent::RenderCompleted);
        }
        result
    }
}

/// Renders the note for a not-applicable case or a case passed under a
/// documented erratum; returns whether the case was rendered.
fn render_conformance_note(
    output: &mut RenderBuffer,
    case: &ConformanceCase,
) -> Result<bool, ArtifactError> {
    if case.disposition == ConformanceDisposition::NotApplicable {
        append_format(
            output,
            format_args!(
                "note[{}]: IRP case {} is not applicable to data model {}\n",
                TerminalText(
                    case.reason_code
                        .as_deref()
                        .unwrap_or(NOT_APPLICABLE_REASON_CODE)
                ),
                TerminalText(&case.id),
                TerminalText(&case.datamodel)
            ),
        )?;
        if let Some(reason) = &case.reason {
            append_format(
                output,
                format_args!("  = reason: {}\n", TerminalText(reason)),
            )?;
        }
        output.push_char('\n')?;
        return Ok(true);
    }
    if let (ConformanceDisposition::Passed, Some(erratum)) = (case.disposition, &case.erratum) {
        append_format(
            output,
            format_args!(
                "note[erratum]: IRP case {} passed under documented erratum {}\n\n",
                TerminalText(&case.id),
                TerminalText(erratum)
            ),
        )?;
        return Ok(true);
    }
    Ok(false)
}

fn render_conformance_text(
    output: &mut RenderBuffer,
    payload: &ConformanceArtifact,
) -> Result<(), ArtifactError> {
    append_format(
        output,
        format_args!(
            "IRP run identity: suite {}@{}; manifest {}; revision {}; corpus {}; source-set {}; schema {:?}; compiler {}@{}; engine {}@{}; data-model {}@{}; runner {}@{}; IR {}\n",
            TerminalText(&payload.identity.suite.id),
            TerminalText(&payload.identity.suite.version),
            TerminalText(&payload.identity.suite.manifest_digest),
            TerminalText(
                payload
                    .identity
                    .suite
                    .revision
                    .as_deref()
                    .unwrap_or("unrecorded")
            ),
            TerminalText(&payload.identity.suite.corpus_digest),
            TerminalText(&payload.identity.suite.source_set_digest),
            payload.identity.schema_policy,
            TerminalText(&payload.identity.compiler.name),
            TerminalText(&payload.identity.compiler.version),
            TerminalText(&payload.identity.engine.name),
            TerminalText(&payload.identity.engine.version),
            TerminalText(&payload.identity.data_model.name),
            TerminalText(&payload.identity.data_model.version),
            TerminalText(&payload.identity.runner.name),
            TerminalText(&payload.identity.runner.version),
            payload.identity.ir_version,
        ),
    )?;
    for case in &payload.cases {
        if render_conformance_note(output, case)? {
            continue;
        }
        if matches!(
            case.disposition,
            ConformanceDisposition::Passed | ConformanceDisposition::Skipped
        ) {
            continue;
        }
        let code = case.reason_code.as_deref().unwrap_or("IRP-NOT-PASSING");
        append_format(
            output,
            format_args!(
                "error[{}]: IRP case {} is {:?} at {}",
                TerminalText(code),
                TerminalText(&case.id),
                case.disposition,
                TerminalText(case.stage.as_str())
            ),
        )?;
        output.push_char('\n')?;
        if let Some(reason) = &case.reason {
            append_format(
                output,
                format_args!("  = reason: {}\n", TerminalText(reason)),
            )?;
        }
        if let Some(erratum) = &case.erratum {
            append_format(
                output,
                format_args!("  = erratum: {}\n", TerminalText(erratum)),
            )?;
        }
        for diagnostic in &case.diagnostics {
            append_format(
                output,
                format_args!(
                    "  = {}[{}]: {}",
                    TerminalText(diagnostic.severity.as_str()),
                    TerminalText(&diagnostic.code),
                    TerminalText(&diagnostic.message)
                ),
            )?;
            output.push_char('\n')?;
        }
        output.push_char('\n')?;
    }
    let counts = payload.summary.counts;
    append_format(
        output,
        format_args!(
            "IRP summary: {}/{} passed ({} under documented errata); {} not passing (failed {}, unsupported {}, invalid {}, harness-error {}); {} skipped; {} not applicable",
            counts.passed,
            counts.applicable_selected,
            counts.passed_under_erratum,
            counts.not_passing,
            counts.failed,
            counts.unsupported,
            counts.invalid,
            counts.harness_error,
            counts.skipped,
            counts.not_applicable
        ),
    )?;
    output.push_char('\n')?;
    Ok(())
}

/// SARIF `tool.driver.semanticVersion` of validator runs: this crate's Cargo
/// package version, the workspace version every producer and renderer shares.
const SARIF_TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Renders a validated canonical artifact as deterministic SARIF 2.1.0 bytes.
///
/// # Errors
///
/// Returns an invariant or serialization failure, or
/// [`ArtifactError::LimitExceeded`] before JSON-tree construction when the
/// validated input exceeds the deterministic SARIF construction budget.
#[allow(
    clippy::too_many_lines,
    reason = "one pass maps the validated envelope to the complete SARIF 2.1.0 document in deterministic field order"
)]
pub fn render_sarif(artifact: &ArtifactEnvelope) -> Result<Vec<u8>, ArtifactError> {
    let span = observe(Module::Reporting, Operation::ReportingRenderSarif);
    if let Err(error) = artifact.check() {
        span.fail(ErrorCategory::Validation);
        return Err(error);
    }
    if let Err(error) = require_sarif_construction_budget(artifact) {
        span.fail(ErrorCategory::Report);
        return Err(error);
    }
    if let ArtifactPayload::Conformance { conformance } = &artifact.payload {
        let result = render_conformance_sarif(conformance);
        if result.is_err() {
            span.fail(ErrorCategory::Report);
        } else {
            span.event(SignalEvent::RenderCompleted);
        }
        return result;
    }
    let (tool_name, source_name, source_text, diagnostics, counts, producer) =
        match &artifact.payload {
            ArtifactPayload::Validator { validator } => (
                "scxml-validator",
                validator.source_name.as_str(),
                validator.source_text.as_str(),
                validator.diagnostics.as_slice(),
                validator.counts,
                json!({"kind": VALIDATOR_KIND}),
            ),
            ArtifactPayload::Compiler { compiler } => (
                "scxml-compiler",
                compiler.source_name.as_str(),
                compiler.source_text.as_str(),
                compiler.diagnostics.as_slice(),
                compiler.counts,
                json!({
                    "kind": COMPILER_KIND,
                    "status": compiler.status,
                    "output": compiler.output,
                }),
            ),
            ArtifactPayload::Conformance { .. } => {
                return Err(ArtifactError::InconsistentConformanceSummary);
            }
        };
    let source = SourceIndex::new(source_text);
    let source_uri = sarif_uri(display_source_name(source_name));

    let codes: std::collections::BTreeSet<&str> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect();
    let rules: Vec<Value> = codes.into_iter().map(sarif_rule).collect();

    let results: Vec<Value> = diagnostics
        .iter()
        .map(|diagnostic| {
            let related_locations: Vec<Value> = diagnostic
                .secondary
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    json!({
                        "id": index + 1,
                        "message": {"text": label.message.as_deref().unwrap_or("related location")},
                        "physicalLocation": physical_location(&source_uri, &source, label.span),
                    })
                })
                .collect();
            let fixes: Vec<Value> = diagnostic
                .fixes
                .iter()
                .map(|fix| {
                    let replacements: Vec<Value> = fix
                        .edits
                        .iter()
                        .map(|edit| {
                            let mut region = sarif_region(&source, edit.span);
                            if let Value::Object(fields) = &mut region {
                                fields.insert("byteOffset".into(), json!(edit.span.start));
                                fields.insert(
                                    "byteLength".into(),
                                    json!(edit.span.end.saturating_sub(edit.span.start)),
                                );
                            }
                            json!({
                                "deletedRegion": region,
                                "insertedContent": {"text": edit.replacement},
                            })
                        })
                        .collect();
                    json!({
                        "description": {"text": fix.message},
                        "artifactChanges": [{
                            "artifactLocation": {"uri": source_uri},
                            "replacements": replacements,
                        }],
                        "properties": {"applicability": fix.applicability},
                    })
                })
                .collect();
            let mut properties = Map::new();
            properties.insert(
                "category".into(),
                Value::String(format!("{:?}", diagnostic.category).to_lowercase()),
            );
            properties.insert(
                "byteSpan".into(),
                json!({"start": diagnostic.primary.span.start, "end": diagnostic.primary.span.end}),
            );
            let mut result = Map::new();
            result.insert("ruleId".into(), Value::String(diagnostic.code.clone()));
            result.insert(
                "level".into(),
                Value::String(match diagnostic.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Note => "note",
                }
                .into()),
            );
            result.insert("message".into(), json!({"text": diagnostic.message}));
            result.insert(
                "locations".into(),
                json!([{"physicalLocation": physical_location(&source_uri, &source, diagnostic.primary.span)}]),
            );
            if !related_locations.is_empty() {
                result.insert("relatedLocations".into(), Value::Array(related_locations));
            }
            if !diagnostic.notes.is_empty()
                || diagnostic.help.is_some()
                || !diagnostic.spec_references.is_empty()
            {
                let mut markdown = vec![diagnostic.message.clone()];
                markdown.extend(
                    diagnostic
                        .notes
                        .iter()
                        .map(|note| format!("**Note:** {note}")),
                );
                if let Some(help) = &diagnostic.help {
                    markdown.push(format!("**Help:** {help}"));
                }
                markdown.extend(
                    diagnostic
                        .spec_references
                        .iter()
                        .map(|reference| format!("**Spec:** {reference}")),
                );
                result.insert(
                    "message".into(),
                    json!({"text": diagnostic.message, "markdown": markdown.join("\n\n")}),
                );
            }
            if !fixes.is_empty() {
                result.insert("fixes".into(), Value::Array(fixes));
            }
            result.insert("properties".into(), Value::Object(properties));
            Value::Object(result)
        })
        .collect();

    let result = render_json_bounded(&json!({
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "columnKind": "utf16CodeUnits",
            "tool": {"driver": {
                "name": tool_name,
                "semanticVersion": SARIF_TOOL_VERSION,
                "informationUri": "https://www.w3.org/TR/scxml/",
                "rules": rules,
            }},
            "artifacts": [{
                "location": {"uri": source_uri},
                "contents": {"text": source_text},
                "mimeType": "application/scxml+xml",
            }],
            "results": results,
            "properties": {"diagnosticCounts": counts, "producer": producer},
        }],
    }));
    if result.is_err() {
        span.fail(ErrorCategory::Report);
    } else {
        span.event(SignalEvent::RenderCompleted);
    }
    result
}

struct SarifConstructionBudget {
    remaining: usize,
}

impl SarifConstructionBudget {
    fn new() -> Self {
        Self {
            remaining: MAX_SARIF_CONSTRUCTION_BYTES,
        }
    }

    fn charge(&mut self, bytes: usize) -> Result<(), ArtifactError> {
        let Some(remaining) = self.remaining.checked_sub(bytes) else {
            return Err(sarif_construction_limit_error());
        };
        self.remaining = remaining;
        Ok(())
    }

    fn charge_text(&mut self, text: &str, copies: usize) -> Result<(), ArtifactError> {
        let Some(bytes) = text.len().checked_mul(copies) else {
            return Err(sarif_construction_limit_error());
        };
        self.charge(bytes)
    }

    fn charge_product(&mut self, left: usize, right: usize) -> Result<(), ArtifactError> {
        let Some(bytes) = left.checked_mul(right) else {
            return Err(sarif_construction_limit_error());
        };
        self.charge(bytes)
    }

    fn charge_node(&mut self) -> Result<(), ArtifactError> {
        // Conservatively covers the fixed `Value` scaffolding, map/vector
        // entries, constant keys, and allocator metadata projected by one
        // input record without depending on target-specific layouts.
        self.charge(4096)
    }
}

fn require_sarif_construction_budget(artifact: &ArtifactEnvelope) -> Result<(), ArtifactError> {
    let mut budget = SarifConstructionBudget::new();
    budget.charge(4096)?;
    match &artifact.payload {
        ArtifactPayload::Validator { validator } => budget_source_sarif(
            &mut budget,
            &validator.source_name,
            &validator.source_text,
            &validator.diagnostics,
        )?,
        ArtifactPayload::Compiler { compiler } => {
            budget_source_sarif(
                &mut budget,
                &compiler.source_name,
                &compiler.source_text,
                &compiler.diagnostics,
            )?;
            if let Some(output) = &compiler.output {
                budget.charge_node()?;
                budget.charge_text(&output.artifact_kind, 1)?;
                budget.charge_text(&output.media_type, 1)?;
                budget.charge_text(&output.file_extension, 1)?;
                budget.charge_text(&output.sha256, 1)?;
                // `serde_json::Value` stores each serialized byte as a
                // number node before final JSON serialization.
                budget.charge_product(output.package_recipe.len(), 32)?;
            }
        }
        ArtifactPayload::Conformance { conformance } => {
            budget_conformance_identity(&mut budget, &conformance.identity)?;
            for key in conformance
                .summary
                .by_spec
                .keys()
                .chain(conformance.summary.by_datamodel.keys())
            {
                budget.charge_node()?;
                budget.charge_text(key, 1)?;
            }
            for case in &conformance.cases {
                budget.charge_node()?;
                for text in [
                    &case.id,
                    &case.assertion_id,
                    &case.spec_num,
                    &case.spec_id,
                    &case.datamodel,
                ] {
                    budget.charge_text(text, 1)?;
                }
                budget.charge_text(case.stage.as_str(), 1)?;
                if let Some(code) = &case.reason_code {
                    budget.charge_text(code, 3)?;
                }
                if let Some(reason) = &case.reason {
                    budget.charge_text(reason, 1)?;
                }
                for diagnostic in &case.diagnostics {
                    budget.charge_node()?;
                    budget.charge_text(&diagnostic.code, 1)?;
                    budget.charge_text(diagnostic.severity.as_str(), 1)?;
                    budget.charge_text(&diagnostic.message, 1)?;
                }
                for state in &case.configuration {
                    budget.charge_node()?;
                    budget.charge_text(state, 1)?;
                }
            }
        }
    }
    Ok(())
}

fn budget_source_sarif(
    budget: &mut SarifConstructionBudget,
    source_name: &str,
    source_text: &str,
    diagnostics: &[Diagnostic],
) -> Result<(), ArtifactError> {
    let uri_uses = diagnostics.iter().try_fold(1_usize, |uses, diagnostic| {
        uses.checked_add(1)
            .and_then(|uses| uses.checked_add(diagnostic.secondary.len()))
            .and_then(|uses| uses.checked_add(diagnostic.fixes.len()))
            .ok_or_else(sarif_construction_limit_error)
    })?;
    budget.charge_product(source_name.len(), 3)?;
    budget.charge_product(
        source_name
            .len()
            .checked_mul(3)
            .ok_or_else(sarif_construction_limit_error)?,
        uri_uses,
    )?;
    budget.charge_text(source_text, 1)?;
    for diagnostic in diagnostics {
        budget.charge_node()?;
        budget.charge_text(&diagnostic.code, 5)?;
        let has_markdown = !diagnostic.notes.is_empty()
            || diagnostic.help.is_some()
            || !diagnostic.spec_references.is_empty();
        budget.charge_text(&diagnostic.message, if has_markdown { 3 } else { 1 })?;
        if let Some(message) = &diagnostic.primary.message {
            budget.charge_text(message, 1)?;
        }
        for label in &diagnostic.secondary {
            budget.charge_node()?;
            if let Some(message) = &label.message {
                budget.charge_text(message, 1)?;
            }
        }
        for note in &diagnostic.notes {
            budget.charge_text(note, 2)?;
        }
        if let Some(help) = &diagnostic.help {
            budget.charge_text(help, 4)?;
        }
        for reference in &diagnostic.spec_references {
            budget.charge_text(reference, 4)?;
        }
        for fix in &diagnostic.fixes {
            budget.charge_node()?;
            budget.charge_text(&fix.message, 2)?;
            for edit in &fix.edits {
                budget.charge_node()?;
                budget.charge_text(&edit.replacement, 1)?;
            }
        }
    }
    Ok(())
}

fn budget_conformance_identity(
    budget: &mut SarifConstructionBudget,
    identity: &ConformanceRunIdentity,
) -> Result<(), ArtifactError> {
    let suite = &identity.suite;
    for text in [
        &suite.id,
        &suite.version,
        &suite.manifest_source,
        &suite.corpus_digest,
        &suite.source_set_digest,
        &suite.manifest_digest,
        &suite.datamodel,
        &identity.report_digest,
    ] {
        budget.charge_text(text, 1)?;
    }
    for implementation in [
        &identity.compiler,
        &identity.engine,
        &identity.data_model,
        &identity.runner,
    ] {
        budget.charge_node()?;
        budget.charge_text(&implementation.name, 1)?;
        budget.charge_text(&implementation.version, 1)?;
        budget.charge_text(&implementation.source_digest, 1)?;
    }
    for text in [
        suite.revision.as_deref(),
        suite.provenance.as_deref(),
        identity.run_revision.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        budget.charge_text(text, 1)?;
    }
    for (name, digest) in &suite.template_digests {
        budget.charge_node()?;
        budget.charge_text(name, 1)?;
        budget.charge_text(digest, 1)?;
    }
    if let Some(transformer) = &suite.transformer {
        budget.charge_node()?;
        budget.charge_text(&transformer.name, 1)?;
        budget.charge_text(&transformer.version, 1)?;
        if let Some(digest) = &transformer.executable_digest {
            budget.charge_text(digest, 1)?;
        }
        for argument in &transformer.command_argv {
            budget.charge_node()?;
            budget.charge_text(argument, 1)?;
        }
    }
    for value in identity
        .filters
        .ids
        .iter()
        .chain(&identity.filters.spec_ids)
        .chain(&identity.filters.spec_nums)
        .chain(&identity.filters.datamodels)
        .chain(&identity.filters.tags)
    {
        budget.charge_node()?;
        budget.charge_text(value, 1)?;
    }
    Ok(())
}

fn sarif_construction_limit_error() -> ArtifactError {
    ArtifactError::LimitExceeded("maximum SARIF construction byte budget")
}

/// SARIF `reportingDescriptor` for one code, from the canonical explain
/// table (static documentation; nothing is revalidated). Codes outside the
/// table get only their identity.
fn sarif_rule(code: &str) -> Value {
    let mut rule = Map::new();
    rule.insert("id".into(), Value::String(code.to_owned()));
    rule.insert("name".into(), Value::String(code.to_owned()));
    let Some(entry) = explain::lookup(code) else {
        return Value::Object(rule);
    };
    rule.insert("shortDescription".into(), json!({"text": entry.title}));
    rule.insert("fullDescription".into(), json!({"text": entry.why}));
    let specification = if entry.spec.is_empty() {
        String::new()
    } else {
        format!("\n\nSpecification: {}", entry.spec)
    };
    let help = format!(
        "How to fix: {}.{specification}\n\nRun `scx --explain {code}` for examples.",
        entry.help
    );
    rule.insert("help".into(), json!({"text": help}));
    if !entry.url.is_empty() {
        rule.insert("helpUri".into(), Value::String(entry.url.to_owned()));
    }
    rule.insert(
        "defaultConfiguration".into(),
        json!({"level": match entry.severity {
            "warning" => "warning",
            "note" => "note",
            _ => "error",
        }}),
    );
    rule.insert("properties".into(), json!({"category": entry.category}));
    Value::Object(rule)
}

fn render_conformance_sarif(payload: &ConformanceArtifact) -> Result<Vec<u8>, ArtifactError> {
    let mut rule_ids = BTreeSet::<String>::new();
    let results: Vec<Value> = payload
        .cases
        .iter()
        .filter(|case| case.disposition != ConformanceDisposition::Passed)
        .map(|case| {
            let code = case
                .reason_code
                .clone()
                .unwrap_or_else(|| match case.disposition {
                    ConformanceDisposition::Skipped => "IRP-SKIPPED".into(),
                    ConformanceDisposition::NotApplicable => NOT_APPLICABLE_REASON_CODE.into(),
                    _ => "IRP-NOT-PASSING".into(),
                });
            rule_ids.insert(code.clone());
            let level = match case.disposition {
                ConformanceDisposition::Skipped | ConformanceDisposition::NotApplicable => "note",
                ConformanceDisposition::Unsupported => "warning",
                _ => "error",
            };
            json!({
                "ruleId": code,
                "level": level,
                "message": {"text": case.reason.clone().unwrap_or_else(|| format!("IRP case {} is {:?}", case.id, case.disposition))},
                "properties": {
                    "caseId": case.id,
                    "assertionId": case.assertion_id,
                    "specNum": case.spec_num,
                    "specId": case.spec_id,
                    "datamodel": case.datamodel,
                    "conformance": case.conformance,
                    "manual": case.manual,
                    "stage": case.stage,
                    "disposition": case.disposition,
                    "diagnostics": case.diagnostics,
                    "configuration": case.configuration,
                    "terminalStatus": case.terminal_status,
                    "commands": case.commands,
                    "fuelConsumed": case.fuel_consumed,
                    "erratum": case.erratum,
                },
            })
        })
        .collect();
    let rules: Vec<Value> = rule_ids
        .iter()
        .map(|code| {
            json!({
                "id": code,
                "name": code,
                "shortDescription": {"text": "Authoritative SCXML IRP case disposition"},
            })
        })
        .collect();
    render_json_bounded(&json!({
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "columnKind": "utf16CodeUnits",
            "tool": {"driver": {
                "name": "scxml-conformance",
                "semanticVersion": payload.identity.runner.version,
                "informationUri": "https://www.w3.org/TR/scxml/",
                "rules": rules,
            }},
            "results": results,
            "properties": {
                "kind": CONFORMANCE_KIND,
                "reportVersion": payload.report_version,
                "identity": payload.identity,
                "summary": payload.summary,
            },
        }],
    }))
}

fn render_json_bounded(value: &Value) -> Result<Vec<u8>, ArtifactError> {
    let mut output = BoundedBytes::new(MAX_RENDERED_BYTES);
    let result = serde_json::to_writer(&mut output, value).map_err(|error| {
        if output.limit_exceeded {
            rendered_limit_error()
        } else {
            ArtifactError::InvalidJson(error.to_string())
        }
    });
    result.map(|()| output.finish())
}

fn validate_span(source: &str, span: ByteSpan) -> Result<(), ArtifactError> {
    let Ok(start) = usize::try_from(span.start) else {
        return Err(ArtifactError::InvalidSpan(span));
    };
    let Ok(end) = usize::try_from(span.end) else {
        return Err(ArtifactError::InvalidSpan(span));
    };
    if start > end
        || end > source.len()
        || !source.is_char_boundary(start)
        || !source.is_char_boundary(end)
    {
        return Err(ArtifactError::InvalidSpan(span));
    }
    Ok(())
}

/// The stable diagnostic code families, each with what its codes report.
/// `SCXML` is reserved for W3C SCXML 1.0 conformance: every `SCXML` code
/// cites the specification section it enforces.
pub const CODE_FAMILIES: &[(&str, &str)] = &[
    (
        "SCXML",
        "W3C SCXML 1.0 conformance; every code cites the section it enforces",
    ),
    ("XML", "XML 1.0 well-formedness and the XML parser's limits"),
    ("XSD", "XML Schema validation of the document"),
    ("COMP", "the compiler"),
    (
        "PKG",
        "extension, service, and data-model packages and their binding",
    ),
    ("ENV", "the built-in `env` attribute vocabulary"),
    (
        "EVENT",
        "the built-in `event` attribute vocabulary (`event:schema`)",
    ),
    (
        "DATAMODEL",
        "the built-in `datamodel` attribute vocabulary (`datamodel:src`)",
    ),
    ("LINT", "best-practice warnings on legal SCXML"),
    (
        "LIMIT",
        "this implementation's bounds, not specification rules",
    ),
];

/// Upper bound on the local part of a package validator's code.
const MAX_PACKAGE_CODE_BYTES: usize = 64;

/// The family of a stable code: its leading ASCII capitals when they name a
/// family of [`CODE_FAMILIES`].
#[must_use]
pub fn code_family(code: &str) -> Option<&'static str> {
    let letters = code
        .find(|character: char| !character.is_ascii_uppercase())
        .map_or(code, |end| &code[..end]);
    CODE_FAMILIES
        .iter()
        .map(|(family, _)| *family)
        .find(|family| *family == letters)
}

/// Whether `code` is a stable diagnostic code: a family of
/// [`CODE_FAMILIES`] followed by exactly three digits, an XML Schema
/// constraint (`XSD-` and the constraint identifier), or a package
/// validator's `<prefix>::<code>` (`<code>` is `[a-z][a-z0-9_]*` within 64
/// bytes). Everything else, an unknown family included, fails closed.
#[must_use]
pub fn is_known_code(code: &str) -> bool {
    if let Some((prefix, local)) = code.split_once("::") {
        return !prefix.is_empty()
            && !prefix
                .chars()
                .any(|character| character == ':' || character.is_whitespace())
            && local.len() <= MAX_PACKAGE_CODE_BYTES
            && local.starts_with(|first: char| first.is_ascii_lowercase())
            && local
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    }
    if let Some(constraint) = code.strip_prefix("XSD-") {
        return !constraint.is_empty()
            && constraint
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    }
    code_family(code).is_some_and(|family| {
        let digits = &code[family.len()..];
        digits.len() == 3 && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn validate_diagnostic_payload(
    source: &str,
    diagnostics: &[Diagnostic],
    counts: Counts,
) -> Result<(), ArtifactError> {
    if scxml_types::exceeds_wasm_bound(source.len(), MAX_SOURCE_BYTES) {
        return Err(ArtifactError::LimitExceeded("maximum source byte length"));
    }
    if scxml_types::exceeds_wasm_bound(diagnostics.len(), MAX_DIAGNOSTICS) {
        return Err(ArtifactError::LimitExceeded("maximum diagnostic count"));
    }
    let expected = Counts::from_diagnostics(diagnostics);
    if counts != expected {
        return Err(ArtifactError::InconsistentCounts {
            declared: counts,
            actual: expected,
        });
    }
    let mut normalized = diagnostics.to_vec();
    normalize_diagnostics(&mut normalized);
    if normalized != diagnostics {
        return Err(ArtifactError::NonCanonicalOrdering);
    }
    for diagnostic in diagnostics {
        if !is_known_code(&diagnostic.code) {
            return Err(ArtifactError::UnknownCode);
        }
        validate_span(source, diagnostic.primary.span)?;
        for label in &diagnostic.secondary {
            validate_span(source, label.span)?;
        }
        for fix in &diagnostic.fixes {
            if fix.edits.is_empty() {
                return Err(ArtifactError::OverlappingEdits);
            }
            let mut previous_end = None;
            for edit in &fix.edits {
                validate_span(source, edit.span)?;
                if previous_end.is_some_and(|end| edit.span.start < end) {
                    return Err(ArtifactError::OverlappingEdits);
                }
                previous_end = Some(edit.span.end);
            }
        }
    }
    Ok(())
}

fn validate_compiler_output(output: &CompilerOutput) -> Result<(), ArtifactError> {
    if scxml_types::exceeds_wasm_bound(output.package_recipe.len(), MAX_PACKAGE_RECIPE_BYTES) {
        return Err(ArtifactError::LimitExceeded(
            "maximum compiler package-recipe byte length",
        ));
    }
    let recipe: Value = serde_json::from_slice(&output.package_recipe)
        .map_err(|_| ArtifactError::NonCanonicalPackageRecipe)?;
    let canonical =
        serde_json::to_vec(&recipe).map_err(|_| ArtifactError::NonCanonicalPackageRecipe)?;
    if canonical != output.package_recipe {
        return Err(ArtifactError::NonCanonicalPackageRecipe);
    }
    let recipe: CompilerPackageRecipe =
        serde_json::from_value(recipe).map_err(|_| ArtifactError::InconsistentCompilerStatus)?;
    let phase = match output.artifact_kind.as_str() {
        "lowered-ir" => CompilerArtifactPhase::LoweredIr,
        "core-wasm" => CompilerArtifactPhase::CoreWasm,
        "component-wasm" => CompilerArtifactPhase::ComponentWasm,
        _ => return Err(ArtifactError::InconsistentCompilerStatus),
    };
    let is_digest = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    };
    let mut canonical_components = recipe.linked_extension_components.clone();
    canonical_components.sort();
    canonical_components.dedup();
    let common = is_digest(&output.sha256)
        && is_digest(&recipe.ir_sha256)
        && recipe.template_world == MACHINE_TEMPLATE_WORLD
        && recipe.template_version == MACHINE_TEMPLATE_VERSION
        && recipe.ir_source_interface == MACHINE_IR_SOURCE_INTERFACE
        && !recipe
            .linked_extension_components
            .iter()
            .any(|component| component.trim().is_empty())
        && recipe.linked_extension_components == canonical_components;
    // Each emitted phase has exactly one honest shape: lowered IR is data
    // whose digest is the IR digest and whose next phase is the component;
    // a component records the pinned template identity and is terminal.
    // Core Wasm is never emitted by this compiler.
    let phase_consistent = match phase {
        CompilerArtifactPhase::LoweredIr => {
            output.media_type == LOWERED_IR_MEDIA_TYPE
                && output.file_extension == LOWERED_IR_FILE_EXTENSION
                && recipe.ir_sha256 == output.sha256
                && recipe.template_sha256.is_none()
                && recipe.next_phase == Some(CompilerArtifactPhase::ComponentWasm)
        }
        CompilerArtifactPhase::ComponentWasm => {
            output.media_type == COMPONENT_WASM_MEDIA_TYPE
                && output.file_extension == COMPONENT_WASM_FILE_EXTENSION
                && recipe.ir_sha256 != output.sha256
                && recipe
                    .template_sha256
                    .as_deref()
                    .is_some_and(|digest| is_digest(digest) && digest != output.sha256)
                && recipe.next_phase.is_none()
        }
        CompilerArtifactPhase::CoreWasm => false,
    };
    if !common || !phase_consistent {
        return Err(ArtifactError::InconsistentCompilerStatus);
    }
    Ok(())
}

fn normalize_conformance_identity(identity: &mut ConformanceRunIdentity) {
    identity.filters.ids.sort();
    identity.filters.ids.dedup();
    identity.filters.id_ranges.sort();
    identity.filters.id_ranges.dedup();
    identity.filters.spec_ids.sort();
    identity.filters.spec_ids.dedup();
    identity.filters.spec_nums.sort();
    identity.filters.spec_nums.dedup();
    identity.filters.conformance.sort();
    identity.filters.conformance.dedup();
    identity.filters.datamodels.sort();
    identity.filters.datamodels.dedup();
    identity.filters.tags.sort();
    identity.filters.tags.dedup();
}

fn validate_conformance_identity(identity: &ConformanceRunIdentity) -> Result<(), ArtifactError> {
    validate_conformance_suite_identity(&identity.suite)?;
    validate_conformance_implementation("compiler", &identity.compiler)?;
    validate_conformance_implementation("engine", &identity.engine)?;
    validate_conformance_implementation("data_model", &identity.data_model)?;
    validate_conformance_implementation("runner", &identity.runner)?;
    validate_conformance_implementation("manual_oracle", &identity.manual_oracle)?;
    require_conformance_collection(
        "maximum conformance extension count",
        identity.extensions.len(),
    )?;
    for extension in &identity.extensions {
        require_conformance_identity_string("extensions.namespace_uri", &extension.namespace_uri)?;
        require_conformance_identity_string(
            "extensions.package_version",
            &extension.package_version,
        )?;
        require_conformance_digest("extensions.manifest_sha256", &extension.manifest_sha256)?;
        require_conformance_digest("extensions.schema_sha256", &extension.schema_sha256)?;
        require_conformance_digest("extensions.component_sha256", &extension.component_sha256)?;
    }
    if !identity
        .extensions
        .windows(2)
        .all(|pair| pair[0].namespace_uri < pair[1].namespace_uri)
    {
        return Err(ArtifactError::NonCanonicalOrdering);
    }
    if identity.ir_version == 0 {
        return Err(ArtifactError::InvalidConformanceIdentity("ir_version"));
    }
    if let Some(run_revision) = &identity.run_revision {
        require_conformance_identity_string("run_revision", run_revision)?;
    }
    require_conformance_digest("report_digest", &identity.report_digest)?;
    validate_conformance_filters(&identity.filters)?;
    let mut normalized = identity.clone();
    normalize_conformance_identity(&mut normalized);
    if normalized != *identity {
        return Err(ArtifactError::NonCanonicalOrdering);
    }
    Ok(())
}

fn validate_conformance_implementation(
    field: &'static str,
    implementation: &ConformanceImplementationIdentity,
) -> Result<(), ArtifactError> {
    require_conformance_identity_string(field, &implementation.name)?;
    require_conformance_identity_string(field, &implementation.version)?;
    require_conformance_digest(field, &implementation.source_digest)
}

fn validate_conformance_suite_identity(
    suite: &ConformanceSuiteIdentity,
) -> Result<(), ArtifactError> {
    require_conformance_identity_string("suite.id", &suite.id)?;
    require_conformance_identity_string("suite.version", &suite.version)?;
    require_conformance_identity_string("suite.manifest_source", &suite.manifest_source)?;
    require_conformance_identity_string("suite.datamodel", &suite.datamodel)?;
    require_conformance_digest("suite.corpus_digest", &suite.corpus_digest)?;
    require_conformance_digest("suite.source_set_digest", &suite.source_set_digest)?;
    require_conformance_digest("suite.manifest_digest", &suite.manifest_digest)?;
    if let Some(revision) = &suite.revision {
        require_conformance_identity_string("suite.revision", revision)?;
    }
    if let Some(provenance) = &suite.provenance {
        require_conformance_identity_string("suite.provenance", provenance)?;
    }
    require_conformance_collection(
        "maximum conformance template identity count",
        suite.template_digests.len(),
    )?;
    for (name, digest) in &suite.template_digests {
        require_conformance_identity_string("suite.template_digests key", name)?;
        require_conformance_digest("suite.template_digests value", digest)?;
    }
    if let Some(transformer) = &suite.transformer {
        require_conformance_identity_string("suite.transformer.name", &transformer.name)?;
        require_conformance_identity_string("suite.transformer.version", &transformer.version)?;
        require_conformance_collection(
            "maximum transformer command argument count",
            transformer.command_argv.len(),
        )?;
        if transformer.command_argv.is_empty() {
            return Err(ArtifactError::InvalidConformanceIdentity(
                "suite.transformer.command_argv",
            ));
        }
        for argument in &transformer.command_argv {
            require_conformance_identity_string(
                "suite.transformer.command_argv argument",
                argument,
            )?;
        }
        if let Some(digest) = &transformer.executable_digest {
            require_conformance_digest("suite.transformer.executable_digest", digest)?;
        }
    }
    if let Some(errata) = &suite.errata {
        require_conformance_identity_string("suite.errata.source", &errata.source)?;
        require_conformance_digest("suite.errata.digest", &errata.digest)?;
        require_conformance_collection(
            "maximum conformance errata count",
            errata
                .applied
                .len()
                .saturating_add(errata.not_applicable.len()),
        )?;
        for (case_id, erratum_id) in &errata.applied {
            require_conformance_identity_string("suite.errata.applied case", case_id)?;
            require_conformance_identity_string("suite.errata.applied erratum", erratum_id)?;
        }
        for case_id in &errata.not_applicable {
            require_conformance_identity_string("suite.errata.not_applicable case", case_id)?;
            if errata.applied.contains_key(case_id) {
                return Err(ArtifactError::InvalidConformanceIdentity(
                    "suite.errata.not_applicable",
                ));
            }
        }
        if !errata.not_applicable.windows(2).all(|pair| {
            (numeric_case_id(&pair[0]), pair[0].as_str())
                < (numeric_case_id(&pair[1]), pair[1].as_str())
        }) {
            return Err(ArtifactError::NonCanonicalOrdering);
        }
        if suite.kind != ConformanceCorpusKind::OfficialIrp {
            return Err(ArtifactError::InvalidConformanceIdentity("suite.errata"));
        }
    }
    if matches!(
        suite.kind,
        ConformanceCorpusKind::OfficialIrp | ConformanceCorpusKind::NamespaceIrp
    ) && (suite.transformer.is_none()
        || suite.template_digests.is_empty()
        || suite.revision.is_none())
    {
        return Err(ArtifactError::InvalidConformanceIdentity(
            "suite.official_irp_provenance",
        ));
    }
    Ok(())
}

fn validate_conformance_filters(filters: &ConformanceFilters) -> Result<(), ArtifactError> {
    for (field, length) in [
        ("maximum conformance filter id count", filters.ids.len()),
        (
            "maximum conformance filter range count",
            filters.id_ranges.len(),
        ),
        (
            "maximum conformance filter spec-id count",
            filters.spec_ids.len(),
        ),
        (
            "maximum conformance filter spec-number count",
            filters.spec_nums.len(),
        ),
        (
            "maximum conformance filter conformance-level count",
            filters.conformance.len(),
        ),
        (
            "maximum conformance filter datamodel count",
            filters.datamodels.len(),
        ),
        ("maximum conformance filter tag count", filters.tags.len()),
    ] {
        require_conformance_collection(field, length)?;
    }
    for value in filters
        .ids
        .iter()
        .chain(&filters.spec_ids)
        .chain(&filters.spec_nums)
        .chain(&filters.datamodels)
        .chain(&filters.tags)
    {
        require_conformance_identity_string("filters.value", value)?;
    }
    if filters
        .id_ranges
        .iter()
        .any(|range| range.start > range.end)
    {
        return Err(ArtifactError::InvalidConformanceIdentity(
            "filters.id_ranges",
        ));
    }
    Ok(())
}

fn require_conformance_identity_string(
    field: &'static str,
    value: &str,
) -> Result<(), ArtifactError> {
    if value.is_empty() || value.len() > MAX_CONFORMANCE_IDENTITY_STRING_BYTES {
        Err(ArtifactError::InvalidConformanceIdentity(field))
    } else {
        Ok(())
    }
}

fn require_conformance_digest(field: &'static str, value: &str) -> Result<(), ArtifactError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(ArtifactError::InvalidConformanceIdentity(field))
    }
}

fn require_conformance_collection(field: &'static str, length: usize) -> Result<(), ArtifactError> {
    if length <= MAX_CONFORMANCE_IDENTITY_ITEMS {
        Ok(())
    } else {
        Err(ArtifactError::LimitExceeded(field))
    }
}

fn add_conformance_count(counts: &mut ConformanceCounts, case: &ConformanceCase) {
    match case.disposition {
        ConformanceDisposition::Passed => {
            counts.applicable_selected += 1;
            counts.passed += 1;
            if case.erratum.is_some() {
                counts.passed_under_erratum += 1;
            }
        }
        ConformanceDisposition::Failed => {
            counts.applicable_selected += 1;
            counts.failed += 1;
            counts.not_passing += 1;
        }
        ConformanceDisposition::Unsupported => {
            counts.applicable_selected += 1;
            counts.unsupported += 1;
            counts.not_passing += 1;
        }
        ConformanceDisposition::Invalid => {
            counts.applicable_selected += 1;
            counts.invalid += 1;
            counts.not_passing += 1;
        }
        ConformanceDisposition::HarnessError => {
            counts.applicable_selected += 1;
            counts.harness_error += 1;
            counts.not_passing += 1;
        }
        ConformanceDisposition::Skipped => counts.skipped += 1,
        ConformanceDisposition::NotApplicable => counts.not_applicable += 1,
    }
}

fn computed_conformance_summary(cases: &[ConformanceCase]) -> ConformanceSummary {
    let mut summary = ConformanceSummary {
        total_selected: cases.len() as u64,
        counts: ConformanceCounts::default(),
        mandatory: ConformanceCounts::default(),
        optional: ConformanceCounts::default(),
        by_spec: BTreeMap::new(),
        by_datamodel: BTreeMap::new(),
    };
    for case in cases {
        add_conformance_count(&mut summary.counts, case);
        add_conformance_count(
            match case.conformance {
                ConformanceLevel::Mandatory => &mut summary.mandatory,
                ConformanceLevel::Optional => &mut summary.optional,
            },
            case,
        );
        add_conformance_count(
            summary.by_spec.entry(case.spec_id.clone()).or_default(),
            case,
        );
        add_conformance_count(
            summary
                .by_datamodel
                .entry(case.datamodel.clone())
                .or_default(),
            case,
        );
    }
    summary
}

/// Computes the lowercase SHA-256 digest over the exact canonical conformance
/// payload with only `identity.report_digest` replaced by the empty string.
///
/// The digest binds the report version, complete run identity, summary, and
/// ordered case sequence. The operation is deterministic and has no side
/// effects beyond bounded telemetry.
///
/// # Errors
///
/// Returns [`ArtifactError::InvalidJson`] if the typed payload cannot be
/// serialized.
pub fn conformance_report_digest(
    conformance: &ConformanceArtifact,
) -> Result<String, ArtifactError> {
    let mut canonical = conformance.clone();
    canonical.identity.report_digest.clear();
    normalize_conformance_identity(&mut canonical.identity);
    let mut writer = ConformanceDigestWriter::new();
    serde_json::to_writer(&mut writer, &canonical).map_err(|error| {
        if writer.limit_exceeded {
            ArtifactError::LimitExceeded("maximum canonical byte length")
        } else {
            ArtifactError::InvalidJson(error.to_string())
        }
    })?;
    Ok(hex::encode(writer.digest.finalize()))
}

struct ConformanceDigestWriter {
    digest: Sha256,
    written: usize,
    limit_exceeded: bool,
}

impl ConformanceDigestWriter {
    fn new() -> Self {
        Self {
            digest: Sha256::new(),
            written: 0,
            limit_exceeded: false,
        }
    }
}

impl io::Write for ConformanceDigestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(next) = self.written.checked_add(bytes.len()) else {
            self.limit_exceeded = true;
            return Err(io::Error::other(
                "canonical conformance payload is too large",
            ));
        };
        if next > MAX_CANONICAL_BYTES {
            self.limit_exceeded = true;
            return Err(io::Error::other(
                "canonical conformance payload is too large",
            ));
        }
        self.digest.update(bytes);
        self.written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn numeric_case_id(id: &str) -> u64 {
    let digits = id.bytes().take_while(u8::is_ascii_digit).count();
    id.get(..digits)
        .and_then(|prefix| prefix.parse().ok())
        .unwrap_or(u64::MAX)
}

/// An oracle kind is present exactly when execution reached oracle
/// evaluation, and its kind matches the corpus provenance: official cases
/// use the `Outcome` log or, for manual cases only, the manual-equivalent
/// oracle; namespace cases use the `Outcome` log or the document-rejection
/// oracle; synthetic cases use only synthetic expectations. A case the
/// document-rejection oracle decided never ran.
fn conformance_case_oracle_is_valid(kind: ConformanceCorpusKind, case: &ConformanceCase) -> bool {
    let evaluated = matches!(
        (case.disposition, case.stage),
        (ConformanceDisposition::Passed, ConformanceStage::Complete)
            | (ConformanceDisposition::Failed, ConformanceStage::Oracle)
    );
    match (kind, case.oracle) {
        (_, None) => !evaluated,
        (
            ConformanceCorpusKind::OfficialIrp | ConformanceCorpusKind::NamespaceIrp,
            Some(ConformanceOracleKind::OutcomeLog),
        )
        | (
            ConformanceCorpusKind::SyntheticHarness,
            Some(ConformanceOracleKind::SyntheticExpectation),
        ) => evaluated && case.terminal_status.is_some(),
        (ConformanceCorpusKind::NamespaceIrp, Some(ConformanceOracleKind::DocumentRejection)) => {
            evaluated
                && case.commands == 0
                && case.fuel_consumed == 0
                && case.terminal_status.is_none()
                && case.configuration.is_empty()
        }
        (ConformanceCorpusKind::OfficialIrp, Some(ConformanceOracleKind::ManualEquivalent)) => {
            evaluated && case.manual && case.terminal_status.is_some()
        }
        _ => false,
    }
}

/// A case's erratum equals the suite errata identity's entry for it, and it
/// is `NotApplicable` exactly when that identity declares it not applicable.
fn conformance_case_errata_is_valid(
    errata: Option<&ConformanceErrataIdentity>,
    case: &ConformanceCase,
) -> bool {
    let recorded = errata.and_then(|errata| errata.applied.get(&case.id));
    let declared = errata.is_some_and(|errata| errata.not_applicable.contains(&case.id));
    case.erratum.as_ref() == recorded
        && declared == (case.disposition == ConformanceDisposition::NotApplicable)
}

fn conformance_case_state_is_valid(case: &ConformanceCase) -> bool {
    let has_reason = case.reason_code.is_some() && case.reason.is_some();
    let no_execution = case.commands == 0
        && case.fuel_consumed == 0
        && case.terminal_status.is_none()
        && case.configuration.is_empty();
    // A rejected document never ran: it has no configuration, commands, or
    // fuel.
    if case.terminal_status == Some(ConformanceTerminalStatus::Rejected)
        && !(case.commands == 0 && case.fuel_consumed == 0 && case.configuration.is_empty())
    {
        return false;
    }
    match (case.disposition, case.stage) {
        // Whether a run must exist is the oracle's rule
        // (`conformance_case_oracle_is_valid`): the document-rejection oracle
        // decides without one.
        (ConformanceDisposition::Passed, ConformanceStage::Complete) => {
            case.reason_code.is_none() && case.reason.is_none()
        }
        (ConformanceDisposition::Skipped, ConformanceStage::Selection) => {
            case.manual && has_reason && no_execution
        }
        (ConformanceDisposition::NotApplicable, ConformanceStage::Selection) => {
            case.reason_code.as_deref() == Some(NOT_APPLICABLE_REASON_CODE)
                && case.reason.is_some()
                && no_execution
                && case.erratum.is_none()
        }
        (ConformanceDisposition::Failed, ConformanceStage::Oracle) => has_reason,
        (
            ConformanceDisposition::Failed
            | ConformanceDisposition::Unsupported
            | ConformanceDisposition::HarnessError,
            ConformanceStage::Execution,
        ) => has_reason && case.terminal_status.is_none() && case.configuration.is_empty(),
        (
            ConformanceDisposition::Unsupported,
            ConformanceStage::Capability | ConformanceStage::Oracle,
        )
        | (
            ConformanceDisposition::Invalid,
            ConformanceStage::Integrity
            | ConformanceStage::Parse
            | ConformanceStage::Validation
            | ConformanceStage::Lowering,
        )
        | (
            ConformanceDisposition::HarnessError,
            ConformanceStage::Preparation
            | ConformanceStage::Integrity
            | ConformanceStage::Lowering,
        ) => has_reason && no_execution,
        _ => false,
    }
}

fn validate_conformance_case(case: &ConformanceCase) -> Result<(), ArtifactError> {
    for (field, value) in [
        ("case.id", case.id.as_str()),
        ("case.assertion_id", case.assertion_id.as_str()),
        ("case.spec_num", case.spec_num.as_str()),
        ("case.spec_id", case.spec_id.as_str()),
        ("case.datamodel", case.datamodel.as_str()),
    ] {
        require_conformance_identity_string(field, value)?;
    }
    require_conformance_collection(
        "maximum conformance diagnostic count",
        case.diagnostics.len(),
    )?;
    require_conformance_collection(
        "maximum conformance configuration count",
        case.configuration.len(),
    )?;
    for diagnostic in &case.diagnostics {
        require_conformance_identity_string("case.diagnostic.code", &diagnostic.code)?;
        if !is_known_code(&diagnostic.code) {
            return Err(ArtifactError::InvalidConformanceCase(
                "case.diagnostic.code",
            ));
        }
        require_conformance_identity_string("case.diagnostic.message", &diagnostic.message)?;
    }
    let mut configuration = BTreeSet::new();
    for state in &case.configuration {
        require_conformance_identity_string("case.configuration", state)?;
        if !configuration.insert(state) {
            return Err(ArtifactError::InvalidConformanceCase(
                "case.configuration duplicate",
            ));
        }
    }
    if let Some(reason_code) = &case.reason_code {
        require_conformance_identity_string("case.reason_code", reason_code)?;
        if !reason_code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(ArtifactError::InvalidConformanceCase("case.reason_code"));
        }
    }
    if let Some(reason) = &case.reason {
        require_conformance_identity_string("case.reason", reason)?;
    }
    if !conformance_case_state_is_valid(case) {
        return Err(ArtifactError::InvalidConformanceCase(
            "case disposition/stage/execution invariant",
        ));
    }
    Ok(())
}

fn validate_conformance(conformance: &ConformanceArtifact) -> Result<(), ArtifactError> {
    if conformance.report_version != CONFORMANCE_REPORT_VERSION {
        return Err(ArtifactError::UnsupportedPayloadVersion {
            kind: CONFORMANCE_KIND,
            version: conformance.report_version,
        });
    }
    if conformance.cases.len() > MAX_CONFORMANCE_CASES {
        return Err(ArtifactError::LimitExceeded(
            "maximum conformance case count",
        ));
    }
    validate_conformance_identity(&conformance.identity)?;
    let mut case_ids = BTreeSet::new();
    for case in &conformance.cases {
        validate_conformance_case(case)?;
        if !conformance_case_oracle_is_valid(conformance.identity.suite.kind, case) {
            return Err(ArtifactError::InvalidConformanceCase(
                "case oracle/disposition/provenance invariant",
            ));
        }
        if !conformance_case_errata_is_valid(conformance.identity.suite.errata.as_ref(), case) {
            return Err(ArtifactError::InvalidConformanceCase(
                "case erratum/not-applicable/errata identity invariant",
            ));
        }
        if !case_ids.insert(case.id.as_str()) {
            return Err(ArtifactError::InvalidConformanceCase("case.id duplicate"));
        }
    }
    if !conformance.cases.windows(2).all(|pair| {
        (numeric_case_id(&pair[0].id), pair[0].id.as_str())
            < (numeric_case_id(&pair[1].id), pair[1].id.as_str())
    }) {
        return Err(ArtifactError::NonCanonicalOrdering);
    }
    if computed_conformance_summary(&conformance.cases) != conformance.summary {
        return Err(ArtifactError::InconsistentConformanceSummary);
    }
    if conformance_report_digest(conformance)? != conformance.identity.report_digest {
        return Err(ArtifactError::ConformanceDigestMismatch);
    }
    Ok(())
}

fn normalize_diagnostics(diagnostics: &mut [Diagnostic]) {
    for diagnostic in diagnostics.iter_mut() {
        diagnostic.secondary.sort();
        diagnostic.notes.sort();
        diagnostic.notes.dedup();
        diagnostic.spec_references.sort();
        diagnostic.spec_references.dedup();
        for fix in &mut diagnostic.fixes {
            fix.edits.sort();
        }
        diagnostic.fixes.sort();
    }
    diagnostics.sort_by(|left, right| {
        (
            left.primary.span,
            left.severity,
            left.code.as_str(),
            left.message.as_str(),
            left.category,
        )
            .cmp(&(
                right.primary.span,
                right.severity,
                right.code.as_str(),
                right.message.as_str(),
                right.category,
            ))
            .then_with(|| left.cmp(right))
    });
}

fn display_source_name(name: &str) -> &str {
    if name.is_empty() { "<input>" } else { name }
}

/// Whether the terminal renderer shows `character` as a visible ASCII escape
/// instead of writing it: C0, DEL, and C1 controls (terminal escape
/// sequences), the U+2028/U+2029 line and paragraph separators (record
/// separators), and the Unicode bidirectional formatting characters (U+061C,
/// U+200E, U+200F, U+202A-U+202E, U+2066-U+2069), which reorder how the
/// surrounding text is displayed (Trojan Source, CVE-2021-42574).
const fn is_escaped_in_terminal(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{007f}'
                | '\u{0080}'..='\u{009f}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// One-line terminal-safe projection of untrusted canonical text.
///
/// Every C0, DEL, and C1 control, the U+2028/U+2029 separators, and every
/// Unicode bidirectional formatting character is rendered as a visible ASCII
/// escape (`\n`, `\r`, `\t`, or `\u{XXXX}`); every other character is
/// written unchanged. Only renderer-owned formatting may therefore emit
/// terminal control bytes or record-separating newlines, and untrusted text
/// cannot reorder how the surrounding output is displayed. The projection is
/// one line and pure ASCII wherever it escapes, so eliding it on a `char`
/// boundary never splits an escape into terminal-visible control bytes.
///
/// Every terminal renderer in the workspace (diagnostics, conformance
/// reports, and the session snapshot `show` and `diff` views) escapes
/// untrusted text through this one type.
pub struct TerminalText<'a>(pub &'a str);

impl fmt::Display for TerminalText<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for character in self.0.chars() {
            match character {
                '\n' => formatter.write_str(r"\n")?,
                '\r' => formatter.write_str(r"\r")?,
                '\t' => formatter.write_str(r"\t")?,
                character if is_escaped_in_terminal(character) => {
                    write!(formatter, "\\u{{{:04X}}}", u32::from(character))?;
                }
                character => formatter.write_char(character)?,
            }
        }
        Ok(())
    }
}

/// Terminal-safe source line with deterministic four-column tab expansion.
struct TerminalSourceLine<'a>(&'a str);

impl fmt::Display for TerminalSourceLine<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut width = 0_usize;
        for character in self.0.chars() {
            if character == '\t' {
                let spaces = 4 - width % 4;
                for _ in 0..spaces {
                    formatter.write_char(' ')?;
                }
                width += spaces;
            } else if is_escaped_in_terminal(character) {
                let mut encoded = [0_u8; 4];
                let escaped = TerminalText(character.encode_utf8(&mut encoded));
                write!(formatter, "{escaped}")?;
                width = width.saturating_add(escaped_terminal_width(character));
            } else {
                formatter.write_char(character)?;
                width = width.saturating_add(character.width().unwrap_or(0));
            }
        }
        Ok(())
    }
}

fn escaped_terminal_width(character: char) -> usize {
    match character {
        '\n' | '\r' | '\t' => 2,
        _ => 8,
    }
}

#[derive(Clone, Copy)]
struct Position {
    line: usize,
    column: usize,
}

struct SourceIndex<'a> {
    text: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> SourceIndex<'a> {
    fn new(text: &'a str) -> Self {
        // A leading UTF-8 byte order mark is not part of line 1: columns and
        // rendered source text start after it, as in editors and rustc, while
        // byte spans keep indexing the original bytes.
        let bom = if text.starts_with('\u{FEFF}') {
            '\u{FEFF}'.len_utf8()
        } else {
            0
        };
        let mut line_starts = vec![bom];
        let bytes = text.as_bytes();
        let mut index = bom;
        while index < bytes.len() {
            match bytes[index] {
                b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                    index += 2;
                    line_starts.push(index);
                }
                b'\r' | b'\n' => {
                    index += 1;
                    line_starts.push(index);
                }
                _ => index += 1,
            }
        }
        Self { text, line_starts }
    }

    fn position(&self, offset: u64) -> Position {
        self.position_with(offset, |text| text.chars().count())
    }

    fn position_utf16(&self, offset: u64) -> Position {
        self.position_with(offset, |text| text.encode_utf16().count())
    }

    fn position_display(&self, offset: u64) -> Position {
        self.position_with(offset, display_width)
    }

    fn position_with(&self, offset: u64, units: impl FnOnce(&str) -> usize) -> Position {
        let offset = usize::try_from(offset)
            .unwrap_or(self.text.len())
            .min(self.text.len());
        let line_index = self
            .line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts.get(line_index).copied().unwrap_or(0);
        let line_prefix = self.text.get(line_start..offset).unwrap_or("");
        Position {
            line: line_index + 1,
            column: units(line_prefix) + 1,
        }
    }

    /// Byte offset of the first byte of one-based `line`.
    fn line_start(&self, line: usize) -> Option<usize> {
        self.line_starts.get(line.checked_sub(1)?).copied()
    }

    /// Byte offset just before the line terminator of one-based `line`.
    fn line_end(&self, line: usize) -> Option<usize> {
        let start = self.line_start(line)?;
        Some(start + self.line(line)?.len())
    }

    fn line(&self, line: usize) -> Option<&str> {
        let start = *self.line_starts.get(line.checked_sub(1)?)?;
        let next = self
            .line_starts
            .get(line)
            .copied()
            .unwrap_or(self.text.len());
        Some(
            self.text[start..next]
                .strip_suffix("\r\n")
                .or_else(|| self.text[start..next].strip_suffix('\n'))
                .or_else(|| self.text[start..next].strip_suffix('\r'))
                .unwrap_or(&self.text[start..next]),
        )
    }
}

fn severity_color(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "31",
        Severity::Warning => "33",
        Severity::Note => "34",
    }
}

fn paint(text: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\u{1b}[{code}m{text}\u{1b}[0m")
    } else {
        text.to_owned()
    }
}

fn display_width(text: &str) -> usize {
    text.chars().fold(0, |width, character| {
        if character == '\t' {
            width + (4 - width % 4)
        } else if is_escaped_in_terminal(character) {
            width.saturating_add(escaped_terminal_width(character))
        } else {
            width.saturating_add(character.width().unwrap_or(0))
        }
    })
}

fn sarif_uri(source_name: &str) -> String {
    let mut uri = String::with_capacity(source_name.len());
    for byte in source_name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            uri.push(char::from(byte));
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            uri.push('%');
            uri.push(char::from(HEX[usize::from(byte >> 4)]));
            uri.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    uri
}

fn physical_location(uri: &str, source: &SourceIndex<'_>, span: ByteSpan) -> Value {
    json!({
        "artifactLocation": {"uri": uri},
        "region": sarif_region(source, span),
    })
}

fn sarif_region(source: &SourceIndex<'_>, span: ByteSpan) -> Value {
    let start = source.position_utf16(span.start);
    let end = source.position_utf16(span.end);
    json!({
        "startLine": start.line,
        "startColumn": start.column,
        "endLine": end.line,
        "endColumn": end.column.max(1),
    })
}

struct RenderBuffer {
    output: String,
    limit: usize,
}

struct BoundedBytes {
    output: Vec<u8>,
    limit: usize,
    limit_exceeded: bool,
}

impl BoundedBytes {
    fn new(limit: usize) -> Self {
        Self {
            output: Vec::new(),
            limit,
            limit_exceeded: false,
        }
    }

    fn finish(self) -> Vec<u8> {
        self.output
    }
}

impl io::Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(next_len) = self.output.len().checked_add(bytes.len()) else {
            self.limit_exceeded = true;
            return Err(io::Error::other("rendered byte limit exceeded"));
        };
        if next_len > self.limit {
            self.limit_exceeded = true;
            return Err(io::Error::other("rendered byte limit exceeded"));
        }
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RenderBuffer {
    fn new(limit: usize) -> Self {
        Self {
            output: String::new(),
            limit,
        }
    }

    fn push_str(&mut self, value: &str) -> Result<(), ArtifactError> {
        self.write_str(value).map_err(|_| rendered_limit_error())
    }

    fn push_char(&mut self, value: char) -> Result<(), ArtifactError> {
        self.write_char(value).map_err(|_| rendered_limit_error())
    }

    fn finish(self) -> String {
        self.output
    }
}

impl fmt::Write for RenderBuffer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let Some(next_len) = self.output.len().checked_add(value.len()) else {
            return Err(fmt::Error);
        };
        if next_len > self.limit {
            return Err(fmt::Error);
        }
        self.output.push_str(value);
        Ok(())
    }
}

fn rendered_limit_error() -> ArtifactError {
    ArtifactError::LimitExceeded("maximum rendered byte length")
}

fn append_format(
    output: &mut RenderBuffer,
    arguments: fmt::Arguments<'_>,
) -> Result<(), ArtifactError> {
    fmt::write(output, arguments).map_err(|_| rendered_limit_error())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Binds the machine template constants recorded in compiler package
    /// recipes to the canonical `stateforward:scxml` WIT package they name:
    /// its package version, its `machine` world, and its `ir-source`
    /// interface. A WIT version bump fails here until the constants follow.
    #[test]
    fn known_codes_are_exactly_the_families_and_package_codes() {
        for code in [
            "SCXML005",
            "XML001",
            "XSD001",
            "COMP001",
            "PKG003",
            "ENV001",
            "EVENT010",
            "LINT021",
            "LIMIT007",
            "XSD-cvc-complex-type.2.4",
            "example::bad_value",
        ] {
            assert!(is_known_code(code), "{code}");
        }
        for code in [
            "",
            "SCXML",
            "SCXML05",
            "SCXML0050",
            "DRAFT001",
            "FOO001",
            "scxml005",
            "LINT01x",
            "XSD-",
            "XSD-a b",
            "::bad",
            "x::",
            "x::Bad",
            "x:y::bad",
        ] {
            assert!(!is_known_code(code), "{code}");
        }
        assert_eq!(code_family("LIMIT001"), Some("LIMIT"));
        assert_eq!(code_family("LINT001"), Some("LINT"));
        assert_eq!(code_family("XSD-cvc-elt"), Some("XSD"));
    }

    #[test]
    fn machine_template_constants_match_the_canonical_wit_package() {
        let wit = include_str!("../../scxml-wit/wit/scxml/scxml.wit");
        let version = wit
            .lines()
            .find_map(|line| line.trim().strip_prefix("package stateforward:scxml@"))
            .and_then(|rest| rest.strip_suffix(';'))
            .expect("scxml.wit declares `package stateforward:scxml@<version>;`");
        assert!(
            wit.lines().any(|line| line.trim() == "world machine {"),
            "scxml.wit declares the `machine` world"
        );
        assert!(
            wit.lines()
                .any(|line| line.trim() == "interface ir-source {"),
            "scxml.wit declares the `ir-source` interface"
        );
        assert_eq!(MACHINE_TEMPLATE_VERSION, version);
        assert_eq!(
            MACHINE_TEMPLATE_WORLD,
            format!("stateforward:scxml/machine@{version}")
        );
        assert_eq!(
            MACHINE_IR_SOURCE_INTERFACE,
            format!("stateforward:scxml/ir-source@{version}")
        );
    }

    fn sample() -> ArtifactEnvelope {
        ArtifactEnvelope::validator(
            "bad<&>.scxml",
            "<scxml>\n  <state id=\"café\"/>\n</scxml>\n",
            vec![Diagnostic {
                severity: Severity::Error,
                code: "SCXML999".into(),
                category: DiagnosticCategory::Semantic,
                message: "broken <state> & target".into(),
                primary: Label {
                    span: ByteSpan { start: 21, end: 26 },
                    message: Some("bad value".into()),
                },
                secondary: vec![Label {
                    span: ByteSpan { start: 2, end: 7 },
                    message: Some("related root".into()),
                }],
                notes: vec!["explain why".into()],
                help: Some("replace the value".into()),
                spec_references: vec!["SCXML 1.0 §3.3".into()],
                fixes: vec![Fix {
                    message: "replace the identifier".into(),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![TextEdit {
                        span: ByteSpan { start: 21, end: 26 },
                        replacement: "ready".into(),
                    }],
                }],
            }],
        )
        .expect("sample validator artifact must satisfy canonical invariants")
    }

    fn lowered_ir_output() -> CompilerOutput {
        CompilerOutput {
            artifact_kind: "lowered-ir".into(),
            media_type: "application/vnd.stateforward.scxml-machine-ir+json".into(),
            file_extension: "scxml-ir.json".into(),
            byte_length: 2,
            sha256: "0".repeat(64),
            package_recipe: serde_json::to_vec(&json!({
                "ir_sha256": "0".repeat(64),
                "ir_source_interface": MACHINE_IR_SOURCE_INTERFACE,
                "linked_extension_components": [],
                "next_phase": "component-wasm",
                "template_sha256": null,
                "template_version": MACHINE_TEMPLATE_VERSION,
                "template_world": MACHINE_TEMPLATE_WORLD,
            }))
            .expect("lowered IR package recipe fixture must serialize"),
        }
    }

    fn component_wasm_output() -> CompilerOutput {
        CompilerOutput {
            artifact_kind: "component-wasm".into(),
            media_type: COMPONENT_WASM_MEDIA_TYPE.into(),
            file_extension: COMPONENT_WASM_FILE_EXTENSION.into(),
            byte_length: 8,
            sha256: "1".repeat(64),
            package_recipe: serde_json::to_vec(&json!({
                "ir_sha256": "0".repeat(64),
                "ir_source_interface": MACHINE_IR_SOURCE_INTERFACE,
                "linked_extension_components": [],
                "next_phase": null,
                "template_sha256": "2".repeat(64),
                "template_version": MACHINE_TEMPLATE_VERSION,
                "template_world": MACHINE_TEMPLATE_WORLD,
            }))
            .expect("component package recipe fixture must serialize"),
        }
    }

    fn with_recipe(mut output: CompilerOutput, field: &str, value: Value) -> CompilerOutput {
        let mut recipe: Value =
            serde_json::from_slice(&output.package_recipe).expect("recipe fixture must parse");
        recipe[field] = value;
        output.package_recipe =
            serde_json::to_vec(&recipe).expect("modified recipe fixture must serialize");
        output
    }

    #[test]
    fn component_wasm_phase_requires_template_identity_and_is_terminal() {
        let emit = |output: CompilerOutput| {
            ArtifactEnvelope::compiler(
                "component-contract.scxml",
                "<scxml/>",
                Vec::new(),
                CompilerStatus::Emitted,
                Some(output),
            )
        };
        let valid = emit(component_wasm_output())
            .expect("a component with pinned template identity must construct");
        assert_eq!(
            decode_canonical(&encode_canonical(&valid).expect("component report encodes")),
            Ok(valid),
            "component-wasm reports round-trip canonically"
        );
        let rejected = [
            with_recipe(component_wasm_output(), "template_sha256", Value::Null),
            with_recipe(component_wasm_output(), "template_sha256", json!("ABC")),
            with_recipe(
                component_wasm_output(),
                "template_sha256",
                json!("1".repeat(64)),
            ),
            with_recipe(
                component_wasm_output(),
                "next_phase",
                json!("component-wasm"),
            ),
            with_recipe(component_wasm_output(), "ir_sha256", json!("1".repeat(64))),
            with_recipe(
                component_wasm_output(),
                "ir_source_interface",
                json!("stateforward:scxml/ir-source@0.3.0"),
            ),
            with_recipe(
                component_wasm_output(),
                "template_world",
                json!("stateforward:scxml/other@0.2.0"),
            ),
            CompilerOutput {
                media_type: LOWERED_IR_MEDIA_TYPE.into(),
                ..component_wasm_output()
            },
            CompilerOutput {
                file_extension: "wasm".into(),
                ..component_wasm_output()
            },
            with_recipe(
                lowered_ir_output(),
                "template_sha256",
                json!("2".repeat(64)),
            ),
            with_recipe(lowered_ir_output(), "next_phase", Value::Null),
        ];
        for output in rejected {
            assert_eq!(
                emit(output),
                Err(ArtifactError::InconsistentCompilerStatus),
                "contradictory component-wasm or lowered-ir phase metadata must fail closed"
            );
        }
    }

    fn conformance_identity() -> ConformanceRunIdentity {
        let implementation = |name: &str, digest: char| ConformanceImplementationIdentity {
            name: name.into(),
            version: "0.1.0".into(),
            source_digest: digest.to_string().repeat(64),
        };
        ConformanceRunIdentity {
            suite: ConformanceSuiteIdentity {
                id: "w3c-irp".into(),
                version: "2015-03-10".into(),
                kind: ConformanceCorpusKind::OfficialIrp,
                manifest_source: "https://www.w3.org/Voice/2013/scxml-irp/manifest.xml".into(),
                revision: Some("w3c-irp-pinned-revision".into()),
                corpus_digest: "a".repeat(64),
                source_set_digest: "b".repeat(64),
                manifest_digest: "c".repeat(64),
                template_digests: BTreeMap::from([("ecmascript".into(), "d".repeat(64))]),
                transformer: Some(ConformanceTransformerIdentity {
                    name: "saxon".into(),
                    version: "Saxon-HE 12".into(),
                    command_argv: vec!["java".into(), "-jar".into(), "saxon-he.jar".into()],
                    executable_digest: Some("e".repeat(64)),
                }),
                datamodel: "ecmascript".into(),
                provenance: Some("W3C Test Suite License".into()),
                errata: None,
            },
            schema_policy: ConformanceSchemaPolicy::Strict,
            compiler: implementation("scxml-compiler-core", '1'),
            engine: implementation("scxml-engine", '2'),
            data_model: implementation("scxml-data-model-ecmascript", '3'),
            runner: implementation("scxml-conformance", '4'),
            manual_oracle: implementation("scxml-conformance-manual-oracle", '5'),
            extensions: Vec::new(),
            ir_version: 1,
            filters: ConformanceFilters {
                ids: vec!["002".into(), "001".into(), "001".into()],
                id_ranges: vec![ConformanceIdRange { start: 1, end: 10 }],
                spec_ids: vec!["5.10".into()],
                spec_nums: Vec::new(),
                conformance: vec![ConformanceLevel::Mandatory],
                datamodels: vec!["ecmascript".into()],
                tags: vec!["mandatory".into()],
            },
            run_revision: Some("0123456789abcdef".into()),
            report_digest: "e".repeat(64),
        }
    }

    #[test]
    fn canonical_bytes_are_deterministic_and_round_trip() {
        let artifact = sample();
        let first = encode_canonical(&artifact).expect("sample artifact must encode");
        let second = encode_canonical(&artifact).expect("sample artifact must re-encode");
        assert_eq!(first, second);
        assert_eq!(
            decode_canonical(&first).expect("canonical sample bytes must decode"),
            artifact
        );
    }

    #[test]
    fn constructor_normalizes_unordered_diagnostic_data() {
        let mut sample = sample();
        let mut later = sample
            .validator_payload_mut()
            .expect("validator payload")
            .diagnostics
            .remove(0);
        later.code = "SCXML998".into();
        later.primary.span = ByteSpan { start: 27, end: 31 };
        later.notes = vec!["z".into(), "a".into(), "a".into()];
        later.spec_references = vec!["z".into(), "a".into(), "a".into()];
        let mut earlier = later.clone();
        earlier.code = "SCXML997".into();
        earlier.primary.span = ByteSpan { start: 2, end: 7 };

        let artifact = ArtifactEnvelope::validator(
            "ordered.scxml",
            "<scxml>\n  <state id=\"café\"/>\n</scxml>\n",
            vec![later, earlier],
        )
        .expect("unordered diagnostic fixture must normalize into an artifact");

        let payload = artifact.validator_payload().expect("validator payload");
        assert_eq!(payload.diagnostics[0].code, "SCXML997");
        assert_eq!(payload.diagnostics[1].notes, ["a", "z"]);
        assert_eq!(payload.diagnostics[1].spec_references, ["a", "z"]);
    }

    #[test]
    fn decoder_rejects_noncanonical_json_spelling() {
        let bytes = encode_canonical(&sample()).expect("sample artifact must encode");
        let mut padded = Vec::with_capacity(bytes.len() + 2);
        padded.push(b' ');
        padded.extend_from_slice(&bytes);
        padded.push(b'\n');
        assert_eq!(
            decode_canonical(&padded),
            Err(ArtifactError::NonCanonicalJson)
        );
    }

    #[test]
    fn rejects_tampered_counts_spans_versions_kinds_and_unknown_fields() {
        let bytes = encode_canonical(&sample()).expect("tamper sample artifact must encode");
        let mut value: Value =
            serde_json::from_slice(&bytes).expect("canonical sample bytes must be valid JSON");
        value["validator"]["counts"]["errors"] = json!(9);
        assert!(matches!(
            decode_canonical(
                &serde_json::to_vec(&value).expect("tampered count JSON must serialize"),
            ),
            Err(ArtifactError::InconsistentCounts { .. })
        ));

        let mut value: Value =
            serde_json::from_slice(&bytes).expect("canonical sample bytes must be valid JSON");
        value["validator"]["diagnostics"][0]["primary"]["span"]["start"] = json!(10_000);
        assert!(matches!(
            decode_canonical(
                &serde_json::to_vec(&value).expect("tampered span JSON must serialize"),
            ),
            Err(ArtifactError::InvalidSpan(_))
        ));

        let mut value: Value =
            serde_json::from_slice(&bytes).expect("canonical sample bytes must be valid JSON");
        value["version"] = json!("scxml.report/v2");
        assert!(matches!(
            decode_canonical(
                &serde_json::to_vec(&value).expect("tampered version JSON must serialize"),
            ),
            Err(ArtifactError::UnsupportedVersion(_))
        ));

        let mut value: Value =
            serde_json::from_slice(&bytes).expect("canonical sample bytes must be valid JSON");
        value["kind"] = json!("unknown");
        assert!(matches!(
            decode_canonical(
                &serde_json::to_vec(&value).expect("tampered kind JSON must serialize"),
            ),
            Err(ArtifactError::UnsupportedKind(_))
        ));

        let mut value: Value =
            serde_json::from_slice(&bytes).expect("canonical sample bytes must be valid JSON");
        value["unexpected"] = json!(true);
        assert!(matches!(
            decode_canonical(
                &serde_json::to_vec(&value).expect("unknown-field JSON must serialize"),
            ),
            Err(ArtifactError::InvalidJson(_))
        ));
    }

    #[test]
    fn compiler_output_rejects_unknown_and_contradictory_phase_metadata() {
        let mut cases = Vec::new();

        let mut unknown = lowered_ir_output();
        unknown.artifact_kind = "future-bytecode".into();
        cases.push(unknown);

        let mut contradictory_media_type = lowered_ir_output();
        contradictory_media_type.media_type = "application/wasm".into();
        cases.push(contradictory_media_type);

        let mut contradictory_extension = lowered_ir_output();
        contradictory_extension.file_extension = "wasm".into();
        cases.push(contradictory_extension);

        let mut unsupported_emitted_phase = lowered_ir_output();
        unsupported_emitted_phase.artifact_kind = "component-wasm".into();
        unsupported_emitted_phase.media_type = "application/wasm".into();
        unsupported_emitted_phase.file_extension = "wasm".into();
        cases.push(unsupported_emitted_phase);

        let mut unsupported_core_phase = lowered_ir_output();
        unsupported_core_phase.artifact_kind = "core-wasm".into();
        unsupported_core_phase.media_type = "application/wasm".into();
        unsupported_core_phase.file_extension = "wasm".into();
        cases.push(unsupported_core_phase);

        let mut contradictory_recipe_phase = lowered_ir_output();
        let mut recipe: Value = serde_json::from_slice(&contradictory_recipe_phase.package_recipe)
            .expect("package recipe fixture must parse");
        recipe["next_phase"] = json!("core-wasm");
        contradictory_recipe_phase.package_recipe =
            serde_json::to_vec(&recipe).expect("modified package recipe fixture must serialize");
        cases.push(contradictory_recipe_phase);

        let mut unknown_recipe_phase = lowered_ir_output();
        let mut recipe: Value = serde_json::from_slice(&unknown_recipe_phase.package_recipe)
            .expect("package recipe fixture must parse");
        recipe["next_phase"] = json!("future-bytecode");
        unknown_recipe_phase.package_recipe =
            serde_json::to_vec(&recipe).expect("modified package recipe fixture must serialize");
        cases.push(unknown_recipe_phase);

        for output in cases {
            assert_eq!(
                ArtifactEnvelope::compiler(
                    "phase-contract.scxml",
                    "<scxml/>",
                    Vec::new(),
                    CompilerStatus::Emitted,
                    Some(output.clone()),
                ),
                Err(ArtifactError::InconsistentCompilerStatus),
                "producers must not construct unknown, unsupported, or contradictory phases"
            );

            let valid = ArtifactEnvelope::compiler(
                "phase-contract.scxml",
                "<scxml/>",
                Vec::new(),
                CompilerStatus::Emitted,
                Some(lowered_ir_output()),
            )
            .expect("valid lowered IR phase contract must construct");
            let mut value: Value = serde_json::from_slice(
                &encode_canonical(&valid).expect("valid compiler fixture must encode"),
            )
            .expect("valid compiler fixture must parse as JSON");
            value["compiler"]["output"] =
                serde_json::to_value(output).expect("tampered compiler output must serialize");
            assert_eq!(
                decode_canonical(
                    &serde_json::to_vec(&value).expect("tampered compiler envelope must serialize")
                ),
                Err(ArtifactError::InconsistentCompilerStatus),
                "the decoder must fail closed on unknown, unsupported, or contradictory phases"
            );
        }
    }

    #[test]
    fn canonical_fix_contract_rejects_empty_edit_lists_for_reporter_parity() {
        let mut artifact = sample();
        artifact
            .validator_payload_mut()
            .expect("validator payload")
            .diagnostics[0]
            .fixes[0]
            .edits
            .clear();

        assert_eq!(artifact.validate(), Err(ArtifactError::OverlappingEdits));
        assert_eq!(
            RustcRenderer::default().render(&artifact),
            Err(ArtifactError::OverlappingEdits)
        );
        assert_eq!(
            render_sarif(&artifact),
            Err(ArtifactError::OverlappingEdits)
        );

        let mut value: Value = serde_json::from_slice(
            &encode_canonical(&sample()).expect("valid fix fixture must encode"),
        )
        .expect("valid fix fixture must parse as JSON");
        value["validator"]["diagnostics"][0]["fixes"][0]["edits"] = json!([]);
        assert_eq!(
            decode_canonical(
                &serde_json::to_vec(&value).expect("empty-fix artifact must serialize")
            ),
            Err(ArtifactError::OverlappingEdits),
            "canonical decoding must reject help-only data disguised as a structured fix"
        );
    }

    #[test]
    fn rustc_and_sarif_both_preserve_canonical_structured_fixes() {
        let artifact = sample();
        let rustc = RustcRenderer::default()
            .render(&artifact)
            .expect("valid structured fix must render as rustc text");
        assert!(
            rustc.contains("help: replace the identifier\n  |\n2 |   <state id=\"ready\"/>\n  |              ~~~~~\n"),
            "{rustc}"
        );

        let sarif: Value = serde_json::from_slice(
            &render_sarif(&artifact).expect("valid structured fix must render as SARIF"),
        )
        .expect("structured-fix SARIF must be valid JSON");
        let fixes = sarif["runs"][0]["results"][0]["fixes"]
            .as_array()
            .expect("SARIF result must preserve canonical structured fixes");
        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0]["artifactChanges"][0]["replacements"][0]["insertedContent"]["text"],
            "ready"
        );
    }

    #[test]
    fn rustc_output_has_unicode_positions_labels_help_and_summary() {
        let rendered = RustcRenderer::default()
            .render(&sample())
            .expect("valid sample must render as rustc text");
        assert!(rendered.contains("bad<&>.scxml:2:14"));
        assert!(rendered.contains("^^^^ bad value"));
        assert!(rendered.contains("----- related root"));
        assert!(rendered.contains("= help: replace the value"));
        assert!(rendered.contains("= spec: SCXML 1.0 §3.3"));
        assert!(rendered.contains("diagnostic summary: 1 error(s)"));
    }

    #[test]
    fn rustc_renderer_revalidates_direct_envelopes_and_emits_failure_telemetry() {
        let recorder = scxml_telemetry::Recorder::default();
        let _guard = scxml_telemetry::install_recorder(recorder.clone());
        let mut artifact = sample();
        let payload = artifact.validator_payload_mut().expect("validator payload");
        payload.diagnostics[0].primary.span = ByteSpan {
            start: 10_000,
            end: 10_001,
        };

        assert!(matches!(
            RustcRenderer::default().render(&artifact),
            Err(ArtifactError::InvalidSpan(_))
        ));
        assert!(recorder.observations().iter().any(|observation| {
            observation.operation == Operation::ReportingRustcRender
                && observation.error == Some(ErrorCategory::Report)
                && !observation.events.contains(&SignalEvent::RenderCompleted)
        }));
    }

    #[test]
    fn rustc_renderer_preserves_complete_multiline_span_extent() {
        let artifact = ArtifactEnvelope::validator(
            "multiline.scxml",
            "zero\none two\nthree four\nfive",
            vec![Diagnostic {
                severity: Severity::Error,
                code: "SCXML995".into(),
                category: DiagnosticCategory::Semantic,
                message: "multiline span".into(),
                primary: Label {
                    span: ByteSpan { start: 7, end: 20 },
                    message: Some("exact extent".into()),
                },
                secondary: Vec::new(),
                notes: Vec::new(),
                help: None,
                spec_references: Vec::new(),
                fixes: Vec::new(),
            }],
        )
        .expect("multiline fixture must satisfy canonical invariants");

        let rendered = RustcRenderer::default()
            .render(&artifact)
            .expect("multiline fixture must render");
        // rustc's multi-line markers: `_` from the margin lane to the first
        // column, the lane down the covered lines, and `|_` to the last.
        assert!(
            rendered
                .contains("2 |   one two\n  |  ___^\n3 | | three four\n  | |_______^ exact extent"),
            "{rendered}"
        );
    }

    #[test]
    fn rustc_renderer_escapes_untrusted_terminal_controls_and_record_breaks() {
        // A code is of a known family, so a hostile code never reaches a
        // renderer: the canonical layer rejects it.
        let hostile_code = ArtifactEnvelope::validator(
            "hostile.scxml",
            "<scxml/>",
            vec![Diagnostic {
                severity: Severity::Error,
                code: "SCXML\u{1b}[2J\nFORGED".into(),
                category: DiagnosticCategory::Semantic,
                message: "message".into(),
                primary: Label {
                    span: ByteSpan { start: 0, end: 1 },
                    message: None,
                },
                secondary: Vec::new(),
                notes: Vec::new(),
                help: None,
                spec_references: Vec::new(),
                fixes: Vec::new(),
            }],
        );
        assert!(matches!(hostile_code, Err(ArtifactError::UnknownCode)));
        let artifact = ArtifactEnvelope::validator(
            "bad\u{1b}[2J\nerror[FORGED].scxml",
            "<scxml>\u{1b}[31m\u{85}payload</scxml>",
            vec![Diagnostic {
                severity: Severity::Error,
                code: "SCXML999".into(),
                category: DiagnosticCategory::Semantic,
                message: "message\nerror[FORGED]: injected\u{9b}31m".into(),
                primary: Label {
                    span: ByteSpan { start: 0, end: 7 },
                    message: Some("label\r\n  = help: forged\u{7}".into()),
                },
                secondary: Vec::new(),
                notes: vec!["note\nwarning[FORGED]\u{1b}]0;owned\u{7}".into()],
                help: Some("help\rerror[FORGED]\u{80}".into()),
                spec_references: vec!["spec\nerror[FORGED]".into()],
                fixes: vec![Fix {
                    message: "fix\nerror[FORGED]".into(),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![TextEdit {
                        span: ByteSpan { start: 0, end: 0 },
                        replacement: "replacement\u{1b}[H\nforged".into(),
                    }],
                }],
            }],
        )
        .expect("hostile terminal fixture must satisfy canonical invariants");

        let rendered = RustcRenderer::default()
            .render(&artifact)
            .expect("hostile terminal fixture must render safely");

        assert!(rendered.contains(r"message\nerror[FORGED]: injected\u{009B}31m"));
        assert!(rendered.contains(r"bad\u{001B}[2J\nerror[FORGED].scxml"));
        assert!(rendered.contains(r"<scxml>\u{001B}[31m\u{0085}payload</scxml>"));
        assert!(!rendered.lines().any(|line| {
            line.starts_with("error[FORGED]") || line.starts_with("warning[FORGED]")
        }));
        assert!(rendered.chars().all(|character| {
            character == '\n'
                || (!character.is_control()
                    && !matches!(character, '\u{007f}' | '\u{0080}'..='\u{009f}'))
        }));

        let colored = RustcRenderer {
            color: ColorChoice::Always,
        }
        .render(&artifact)
        .expect("hostile terminal fixture must render with owned color safely");
        let without_owned_color = ["31", "32", "33", "34", "36", "0"]
            .iter()
            .fold(colored, |text, code| {
                text.replace(&format!("\u{1b}[{code}m"), "")
            });
        assert!(!without_owned_color.contains('\u{1b}'));
    }

    #[test]
    fn rustc_renderer_escapes_bidi_controls_and_underlines_their_escapes() {
        // Trojan Source (CVE-2021-42574): every Unicode bidirectional
        // formatting character in untrusted text is shown as an escape, so a
        // diagnostic displays in logical order, and the caret run under a
        // value covers the escape the snippet actually shows.
        let bidi = [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
            '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ];
        let value: String = bidi.iter().collect();
        let source = format!("<t a=\"zz\u{202e}DER\"/><u b=\"{value}\"/>");
        let start = u64::try_from(source.find("zz").expect("fixture has the value"))
            .expect("offset fits u64");
        let end = start + u64::try_from("zz\u{202e}DER".len()).expect("length fits u64");
        let artifact = ArtifactEnvelope::validator(
            "bidi\u{2066}.scxml",
            source,
            vec![Diagnostic {
                severity: Severity::Error,
                code: "SCXML005".into(),
                category: DiagnosticCategory::Semantic,
                message: format!("unknown target `zz\u{202e}DER` and `{value}`"),
                primary: Label {
                    span: ByteSpan { start, end },
                    message: Some("target \u{2067}here".into()),
                },
                secondary: Vec::new(),
                notes: vec!["note \u{200f}".into()],
                help: Some("help \u{061c}".into()),
                spec_references: Vec::new(),
                fixes: Vec::new(),
            }],
        )
        .expect("bidi fixture must satisfy canonical invariants");

        let rendered = RustcRenderer::default()
            .render(&artifact)
            .expect("bidi fixture must render");

        for character in bidi {
            assert!(
                !rendered.contains(character),
                "U+{:04X} is written raw:\n{rendered}",
                u32::from(character)
            );
        }
        assert!(
            rendered.contains(r"unknown target `zz\u{202E}DER` and `\u{061C}\u{200E}"),
            "{rendered}"
        );
        assert!(rendered.contains(r"bidi\u{2066}.scxml"), "{rendered}");
        assert!(rendered.contains(r"target \u{2067}here"), "{rendered}");
        assert!(rendered.contains(r"= note: note \u{200F}"), "{rendered}");
        assert!(rendered.contains(r"= help: help \u{061C}"), "{rendered}");
        // `zz` + the 8-column escape + `DER`: 13 carets under 13 columns.
        let snippet = r#"<t a="zz\u{202E}DER"/>"#;
        let line = rendered
            .lines()
            .find(|line| line.contains(snippet))
            .expect("the source line is shown with the escape");
        let column = line.find(snippet).expect("snippet is on the line") + 6;
        let carets = rendered
            .lines()
            .skip_while(|candidate| *candidate != line)
            .nth(1)
            .expect("an underline row follows the source line");
        assert_eq!(
            carets.get(column..column + 14),
            Some("^^^^^^^^^^^^^ "),
            "{rendered}"
        );
    }

    fn multi_edit_fixture(replacement: &str) -> ArtifactEnvelope {
        let source = (1..=30).fold(String::new(), |mut text, line| {
            writeln!(text, "line{line:02}").expect("writing to a String cannot fail");
            text
        });
        let span_of = |needle: &str| {
            let start = u64::try_from(source.find(needle).expect("fixture line exists"))
                .expect("offset fits u64");
            ByteSpan {
                start,
                end: start + u64::try_from(needle.len()).expect("length fits u64"),
            }
        };
        let edit = |needle: &str| TextEdit {
            span: span_of(needle),
            replacement: replacement.into(),
        };
        let diagnostic = Diagnostic {
            severity: Severity::Error,
            code: "LINT015".into(),
            category: DiagnosticCategory::Semantic,
            message: "far edits".into(),
            primary: Label {
                span: span_of("line02"),
                message: None,
            },
            secondary: Vec::new(),
            notes: Vec::new(),
            help: None,
            spec_references: Vec::new(),
            fixes: vec![Fix {
                message: "edit far apart".into(),
                applicability: Applicability::MaybeIncorrect,
                // Lines 2 and 4 share a block (one unchanged line between),
                // line 20 and the deletion of lines 28-29 are their own.
                edits: vec![
                    edit("line02"),
                    edit("line04"),
                    edit("line20"),
                    edit("line28\nline29\n"),
                ],
            }],
        };
        ArtifactEnvelope::validator("far.scxml", source.clone(), vec![diagnostic])
            .expect("multi-edit fixture must satisfy canonical invariants")
    }

    #[test]
    fn rustc_renderer_shows_every_block_of_a_multi_edit_fix() {
        // Replacements: every block is shown, `...` separates blocks, and a
        // single unchanged line between two edits is shown, not elided.
        let rendered = RustcRenderer::default()
            .render(&multi_edit_fixture("renamed"))
            .expect("multi-edit fixture must render");
        let suggestion = rendered
            .split_once("help: edit far apart\n")
            .expect("the suggestion is rendered")
            .1;
        assert!(
            suggestion.starts_with(
                "   |\n 2 | renamed\n   | ~~~~~~~\n 3 | line03\n 4 | renamed\n   | ~~~~~~~\n...\n20 | renamed\n   | ~~~~~~~\n...\n28 | renamedline30\n   | ~~~~~~~\n"
            ),
            "{rendered}"
        );

        // A deleting fix: each block is a diff of its lines, without the
        // unchanged lines at its ends (line 30 after the last deletion).
        let rendered = RustcRenderer::default()
            .render(&multi_edit_fixture(""))
            .expect("deletion fixture must render");
        let suggestion = rendered
            .split_once("help: edit far apart\n")
            .expect("the suggestion is rendered")
            .1;
        assert!(
            suggestion.starts_with(
                "   |\n 2 - line02\n 3 - line03\n 4 - line04\n 2 + \n 3 + line03\n 4 + \n...\n20 - line20\n20 + \n...\n28 - line28\n29 - line29\n\n"
            ),
            "{rendered}"
        );
        assert!(
            !suggestion.contains("line05"),
            "unchanged lines between blocks are elided: {rendered}"
        );
    }

    #[test]
    fn rustc_renderer_marks_elided_suggestion_lines_and_blocks() {
        // One block longer than the line cap ends with an explicit `...`.
        let long = (0..20).fold(String::new(), |mut text, index| {
            writeln!(text, "x{index}").expect("writing to a String cannot fail");
            text
        });
        let source = format!("a\n{long}b\n");
        let artifact = ArtifactEnvelope::validator(
            "long.scxml",
            source,
            vec![Diagnostic {
                severity: Severity::Error,
                code: "XML001".into(),
                category: DiagnosticCategory::Syntax,
                message: "long deletion".into(),
                primary: Label {
                    span: ByteSpan { start: 0, end: 1 },
                    message: None,
                },
                secondary: Vec::new(),
                notes: Vec::new(),
                help: None,
                spec_references: Vec::new(),
                fixes: vec![Fix {
                    message: "delete".into(),
                    applicability: Applicability::MaybeIncorrect,
                    edits: vec![TextEdit {
                        span: ByteSpan {
                            start: 2,
                            end: 2 + u64::try_from(long.len()).expect("length fits u64"),
                        },
                        replacement: String::new(),
                    }],
                }],
            }],
        )
        .expect("long deletion fixture must satisfy canonical invariants");
        let rendered = RustcRenderer::default()
            .render(&artifact)
            .expect("long deletion fixture must render");
        assert!(rendered.contains("13 - x11\n...\n"), "{rendered}");
        assert!(!rendered.contains("x12"), "{rendered}");
    }

    /// Natively the diagnostic count is unbounded: one past the wasm bound
    /// (100,000) still forms a canonical validator envelope.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_envelopes_have_no_diagnostic_count_bound() {
        let diagnostics = (0..100_001_u64)
            .map(|offset| Diagnostic {
                severity: Severity::Warning,
                code: "LINT005".into(),
                category: DiagnosticCategory::Lint,
                message: "unreachable".into(),
                primary: Label {
                    span: ByteSpan {
                        start: offset,
                        end: offset + 1,
                    },
                    message: None,
                },
                secondary: Vec::new(),
                notes: Vec::new(),
                help: None,
                spec_references: Vec::new(),
                fixes: Vec::new(),
            })
            .collect();
        let artifact = ArtifactEnvelope::validator("many.scxml", "x".repeat(100_002), diagnostics)
            .expect("native envelopes carry any number of diagnostics");
        assert_eq!(
            artifact
                .validator_payload()
                .expect("validator payload")
                .diagnostics
                .len(),
            100_001
        );
    }

    #[test]
    fn rustc_renderer_rejects_output_amplification_without_truncation() {
        // A tab disables long-line windowing (tab stops depend on the whole
        // prefix), so the full 12 MiB line is rendered per diagnostic and the
        // output bound must reject it rather than truncate silently.
        let source = format!("\t{}", "x".repeat(12 * 1024 * 1024));
        let diagnostics = (1_u64..4)
            .map(|offset| Diagnostic {
                severity: Severity::Error,
                code: format!("SCXML99{offset}"),
                category: DiagnosticCategory::Semantic,
                message: "bounded amplification".into(),
                primary: Label {
                    span: ByteSpan {
                        start: offset,
                        end: offset + 1,
                    },
                    message: None,
                },
                secondary: Vec::new(),
                notes: Vec::new(),
                help: None,
                spec_references: Vec::new(),
                fixes: Vec::new(),
            })
            .collect();
        let artifact = ArtifactEnvelope::validator("wide.scxml", source, diagnostics)
            .expect("amplification fixture must satisfy canonical input bounds");
        assert_eq!(
            RustcRenderer::default().render(&artifact),
            Err(ArtifactError::LimitExceeded("maximum rendered byte length"))
        );
    }

    #[test]
    fn rustc_renderer_windows_long_lines_and_their_suggestions() {
        let source = format!("{}<bad/>{}", "a".repeat(4096), "z".repeat(4096));
        let diagnostics = vec![Diagnostic {
            severity: Severity::Error,
            code: "SCXML995".into(),
            category: DiagnosticCategory::Semantic,
            message: "long line".into(),
            primary: Label {
                span: ByteSpan {
                    start: 4097,
                    end: 4100,
                },
                message: Some("here".into()),
            },
            secondary: Vec::new(),
            notes: Vec::new(),
            help: None,
            spec_references: Vec::new(),
            fixes: vec![Fix {
                message: "rename".into(),
                applicability: Applicability::MachineApplicable,
                edits: vec![TextEdit {
                    span: ByteSpan {
                        start: 4097,
                        end: 4100,
                    },
                    replacement: "good".into(),
                }],
            }],
        }];
        let artifact = ArtifactEnvelope::validator("long.scxml", source, diagnostics)
            .expect("long-line fixture must satisfy canonical input bounds");
        let rendered = RustcRenderer::default()
            .render(&artifact)
            .expect("a long line must render as a window");
        assert!(rendered.len() < 2048, "{rendered}");
        assert!(
            rendered.contains("...aaaaaaaaaaaaaaaaaaaaaaa<bad/>zzz"),
            "{rendered}"
        );
        assert!(rendered.contains("<good/>zzz"), "{rendered}");
        assert!(rendered.contains("~~~~"), "{rendered}");
    }

    #[test]
    fn sarif_is_parseable_escaped_and_deterministic() {
        let first = render_sarif(&sample()).expect("sample artifact must render as SARIF");
        let second = render_sarif(&sample()).expect("sample artifact must rerender as SARIF");
        assert_eq!(first, second);
        let value: Value =
            serde_json::from_slice(&first).expect("rendered SARIF must be valid JSON");
        assert_eq!(value["version"], "2.1.0");
        assert_eq!(value["runs"][0]["results"][0]["ruleId"], "SCXML999");
        assert_eq!(
            value["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"]["startLine"],
            2
        );
        assert_eq!(
            value["runs"][0]["artifacts"][0]["location"]["uri"],
            "bad%3C%26%3E.scxml"
        );
    }

    #[test]
    fn unicode_and_tabs_use_character_header_terminal_carets_and_utf16_sarif() {
        let artifact = ArtifactEnvelope::validator(
            "emoji <é>.scxml",
            "😀\tx",
            vec![Diagnostic {
                severity: Severity::Error,
                code: "SCXML996".into(),
                category: DiagnosticCategory::Semantic,
                message: "unicode span".into(),
                primary: Label {
                    span: ByteSpan { start: 5, end: 6 },
                    message: Some("x".into()),
                },
                secondary: Vec::new(),
                notes: Vec::new(),
                help: None,
                spec_references: Vec::new(),
                fixes: Vec::new(),
            }],
        )
        .expect("unicode diagnostic fixture must produce an artifact");

        let rendered = RustcRenderer::default()
            .render(&artifact)
            .expect("valid unicode artifact must render as rustc text");
        // The header reports the character column (as rustc does); only the
        // caret line uses terminal display width.
        assert!(rendered.contains("emoji <é>.scxml:1:3"), "{rendered}");
        assert!(rendered.contains("1 | 😀  x\n  |     ^ x"));

        let sarif_bytes = render_sarif(&artifact).expect("unicode artifact must render as SARIF");
        let sarif: Value =
            serde_json::from_slice(&sarif_bytes).expect("unicode SARIF must be valid JSON");
        let region = &sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startColumn"], 4);
        assert_eq!(region["endColumn"], 5);
        assert_eq!(sarif["runs"][0]["columnKind"], "utf16CodeUnits");
        assert_eq!(
            sarif["runs"][0]["artifacts"][0]["location"]["uri"],
            "emoji%20%3C%C3%A9%3E.scxml"
        );
    }

    #[test]
    fn sarif_run_normatively_declares_utf16_code_unit_columns() {
        let bytes = render_sarif(&sample()).expect("sample artifact must render as SARIF");
        let document: Value =
            serde_json::from_slice(&bytes).expect("rendered SARIF must be valid JSON");
        let runs = document["runs"]
            .as_array()
            .expect("SARIF log must contain a runs array");
        assert!(!runs.is_empty(), "SARIF log must contain at least one run");
        assert!(runs.iter().all(|run| {
            run.as_object()
                .and_then(|object| object.get("columnKind"))
                .and_then(Value::as_str)
                == Some("utf16CodeUnits")
        }));
    }

    #[test]
    fn sarif_tool_version_is_the_cargo_package_version() {
        let workspace = include_str!("../../../Cargo.toml");
        let declared = workspace
            .lines()
            .find_map(|line| line.strip_prefix("version = \""))
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("the workspace manifest declares the package version");
        assert_eq!(SARIF_TOOL_VERSION, declared);
        let bytes = render_sarif(&sample()).expect("sample artifact must render as SARIF");
        let document: Value =
            serde_json::from_slice(&bytes).expect("rendered SARIF must be valid JSON");
        assert_eq!(
            document["runs"][0]["tool"]["driver"]["semanticVersion"],
            Value::from(declared)
        );
    }

    #[test]
    fn sarif_renderer_rejects_output_amplification_without_truncation() {
        let recorder = scxml_telemetry::Recorder::default();
        let _guard = scxml_telemetry::install_recorder(recorder.clone());
        let large_message = "m".repeat(1024 * 1024);
        let source = "x".repeat(17);
        let diagnostics = (0_u64..17)
            .map(|offset| Diagnostic {
                severity: Severity::Error,
                code: format!("SCXML{offset:03}"),
                category: DiagnosticCategory::Semantic,
                message: large_message.clone(),
                primary: Label {
                    span: ByteSpan {
                        start: offset,
                        end: offset + 1,
                    },
                    message: None,
                },
                secondary: Vec::new(),
                notes: vec!["force markdown rendering".into()],
                help: None,
                spec_references: Vec::new(),
                fixes: Vec::new(),
            })
            .collect();
        let artifact = ArtifactEnvelope::validator("messages.scxml", source, diagnostics)
            .expect("SARIF amplification fixture must satisfy canonical input bounds");
        encode_canonical(&artifact)
            .expect("SARIF amplification fixture must fit the canonical byte budget");

        assert_eq!(
            render_sarif(&artifact),
            Err(ArtifactError::LimitExceeded(
                "maximum SARIF construction byte budget"
            ))
        );
        assert!(recorder.observations().iter().any(|observation| {
            observation.operation == Operation::ReportingRenderSarif
                && observation.error == Some(ErrorCategory::Report)
                && !observation.events.contains(&SignalEvent::RenderCompleted)
        }));
    }

    #[test]
    fn sarif_renderer_rejects_peak_construction_amplification_before_cloning() {
        let large_message = "m".repeat(11 * 1024 * 1024);
        let artifact = ArtifactEnvelope::validator(
            "peak.scxml",
            "x",
            vec![Diagnostic {
                severity: Severity::Error,
                code: "SCXML999".into(),
                category: DiagnosticCategory::Semantic,
                message: large_message,
                primary: Label {
                    span: ByteSpan { start: 0, end: 1 },
                    message: None,
                },
                secondary: Vec::new(),
                notes: vec!["include markdown projection".into()],
                help: None,
                spec_references: Vec::new(),
                fixes: Vec::new(),
            }],
        )
        .expect("peak-construction fixture must satisfy canonical invariants");
        encode_canonical(&artifact)
            .expect("peak-construction fixture must fit the canonical byte budget");

        assert_eq!(
            render_sarif(&artifact),
            Err(ArtifactError::LimitExceeded(
                "maximum SARIF construction byte budget"
            ))
        );
    }

    #[test]
    fn rule_ids_are_unique_in_sarif_inventory() {
        let mut artifact = sample();
        let payload = artifact.validator_payload_mut().expect("validator payload");
        payload.diagnostics.push(payload.diagnostics[0].clone());
        payload.counts = Counts::from_diagnostics(&payload.diagnostics);
        let sarif_bytes =
            render_sarif(&artifact).expect("duplicate-rule artifact must render as SARIF");
        let sarif: Value =
            serde_json::from_slice(&sarif_bytes).expect("duplicate-rule SARIF must be valid JSON");
        let rules = sarif["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .expect("SARIF rules inventory must be an array");
        let ids: BTreeSet<_> = rules
            .iter()
            .map(|rule| {
                rule["id"]
                    .as_str()
                    .expect("every SARIF rule must have a string ID")
            })
            .collect();
        assert_eq!(rules.len(), ids.len());
    }

    #[test]
    fn reporting_control_paths_emit_telemetry() {
        let recorder = scxml_telemetry::Recorder::default();
        let _guard = scxml_telemetry::install_recorder(recorder.clone());
        let artifact = sample();

        let canonical = encode_canonical(&artifact).expect("telemetry artifact must encode");
        let decoded = decode_canonical(&canonical).expect("telemetry artifact bytes must decode");
        let _rustc = RustcRenderer::default()
            .render(&decoded)
            .expect("decoded telemetry artifact must render as rustc text");
        let _sarif =
            render_sarif(&decoded).expect("decoded telemetry artifact must render as SARIF");

        let observations = recorder.observations();
        assert!(
            observations.len() >= 4,
            "canonical encode/decode and both reporters must open OTEL spans"
        );
        assert!(
            observations
                .iter()
                .any(|observation| { observation.events.contains(&SignalEvent::ArtifactAccepted) })
        );
        assert!(
            observations
                .iter()
                .flat_map(|observation| observation.events.iter())
                .filter(|event| **event == SignalEvent::RenderCompleted)
                .count()
                >= 2
        );
    }

    #[test]
    fn compiler_payload_round_trips_and_reporters_preserve_producer_status() {
        let validator = sample();
        let diagnostics = validator
            .validator_payload()
            .expect("validator payload")
            .diagnostics
            .clone();
        let artifact = ArtifactEnvelope::compiler(
            "machine.scxml",
            "<scxml>\n  <state id=\"café\"/>\n</scxml>\n",
            diagnostics,
            CompilerStatus::Rejected,
            None,
        )
        .expect("compiler fixture must produce an artifact");
        let bytes = encode_canonical(&artifact).expect("compiler artifact must encode");
        assert_eq!(
            decode_canonical(&bytes).expect("compiler artifact bytes must decode"),
            artifact
        );
        assert!(
            RustcRenderer::default()
                .render(&artifact)
                .expect("compiler artifact must render as rustc text")
                .contains("Rejected")
        );
        let sarif_bytes = render_sarif(&artifact).expect("compiler artifact must render as SARIF");
        let sarif: Value =
            serde_json::from_slice(&sarif_bytes).expect("compiler SARIF must be valid JSON");
        assert_eq!(
            sarif["runs"][0]["properties"]["producer"]["status"],
            "rejected"
        );
    }

    fn oracle_case(
        disposition: ConformanceDisposition,
        stage: ConformanceStage,
        manual: bool,
        oracle: Option<ConformanceOracleKind>,
    ) -> ConformanceCase {
        ConformanceCase {
            id: "178".into(),
            assertion_id: "178".into(),
            spec_num: "6.2".into(),
            spec_id: "#send".into(),
            datamodel: "ecmascript".into(),
            conformance: ConformanceLevel::Mandatory,
            manual,
            stage,
            disposition,
            reason_code: None,
            reason: None,
            diagnostics: Vec::new(),
            configuration: vec!["final".into()],
            terminal_status: Some(ConformanceTerminalStatus::Finished),
            commands: 1,
            fuel_consumed: 1,
            oracle,
            erratum: None,
        }
    }

    #[test]
    fn conformance_oracle_kind_is_bound_to_disposition_manual_flag_and_provenance() {
        let official = ConformanceCorpusKind::OfficialIrp;
        let synthetic = ConformanceCorpusKind::SyntheticHarness;
        let passed = (ConformanceDisposition::Passed, ConformanceStage::Complete);
        let failed = (ConformanceDisposition::Failed, ConformanceStage::Oracle);
        let unsupported = (
            ConformanceDisposition::Unsupported,
            ConformanceStage::Oracle,
        );
        let manual = Some(ConformanceOracleKind::ManualEquivalent);
        let outcome = Some(ConformanceOracleKind::OutcomeLog);
        let expectation = Some(ConformanceOracleKind::SyntheticExpectation);
        let accepted = [
            (official, passed, true, manual),
            (official, failed, true, manual),
            (official, passed, false, outcome),
            (official, failed, true, outcome),
            (synthetic, passed, false, expectation),
            (official, unsupported, true, None),
        ];
        for (kind, (disposition, stage), is_manual, oracle) in accepted {
            assert!(
                conformance_case_oracle_is_valid(
                    kind,
                    &oracle_case(disposition, stage, is_manual, oracle)
                ),
                "{kind:?} {disposition:?} manual={is_manual} {oracle:?} must be accepted"
            );
        }
        let rejected = [
            // A decided case must name its oracle.
            (official, passed, false, None),
            (official, failed, false, None),
            // Only official manual cases may use the manual-equivalent oracle.
            (official, passed, false, manual),
            (synthetic, passed, true, manual),
            // Official and synthetic oracles never cross provenance.
            (synthetic, passed, false, outcome),
            (official, passed, false, expectation),
            // An undecided case has no oracle.
            (official, unsupported, true, manual),
        ];
        for (kind, (disposition, stage), is_manual, oracle) in rejected {
            assert!(
                !conformance_case_oracle_is_valid(
                    kind,
                    &oracle_case(disposition, stage, is_manual, oracle)
                ),
                "{kind:?} {disposition:?} manual={is_manual} {oracle:?} must be rejected"
            );
        }
    }

    /// A namespace case is decided by the `Outcome` log after a run, or by
    /// the document-rejection oracle without one; neither crosses into
    /// another provenance.
    #[test]
    fn namespace_oracles_are_bound_to_their_provenance_and_to_whether_a_run_exists() {
        let namespace = ConformanceCorpusKind::NamespaceIrp;
        let rejection = Some(ConformanceOracleKind::DocumentRejection);
        let outcome = Some(ConformanceOracleKind::OutcomeLog);
        let never_ran = |disposition, stage| ConformanceCase {
            configuration: Vec::new(),
            terminal_status: None,
            commands: 0,
            fuel_consumed: 0,
            ..oracle_case(disposition, stage, false, rejection)
        };
        for (disposition, stage) in [
            (ConformanceDisposition::Passed, ConformanceStage::Complete),
            (ConformanceDisposition::Failed, ConformanceStage::Oracle),
        ] {
            let case = never_ran(disposition, stage);
            assert!(conformance_case_oracle_is_valid(namespace, &case));
            assert!(conformance_case_state_is_valid(&ConformanceCase {
                reason_code: (disposition == ConformanceDisposition::Failed)
                    .then(|| "document-accepted".into()),
                reason: (disposition == ConformanceDisposition::Failed).then(|| "accepted".into()),
                ..case.clone()
            }));
            for kind in [
                ConformanceCorpusKind::OfficialIrp,
                ConformanceCorpusKind::SyntheticHarness,
            ] {
                assert!(!conformance_case_oracle_is_valid(kind, &case), "{kind:?}");
            }
            // A run never accompanies the rejection oracle, and always the
            // Outcome oracle.
            let ran = oracle_case(disposition, stage, false, rejection);
            assert!(!conformance_case_oracle_is_valid(namespace, &ran));
            assert!(conformance_case_oracle_is_valid(
                namespace,
                &oracle_case(disposition, stage, false, outcome)
            ));
            assert!(!conformance_case_oracle_is_valid(
                namespace,
                &ConformanceCase {
                    oracle: outcome,
                    ..case
                }
            ));
        }
        assert!(!conformance_case_oracle_is_valid(
            namespace,
            &oracle_case(
                ConformanceDisposition::Passed,
                ConformanceStage::Complete,
                true,
                Some(ConformanceOracleKind::ManualEquivalent)
            )
        ));
    }

    #[test]
    fn conformance_constructor_rejects_a_passed_case_without_an_oracle_kind() {
        let cases = vec![oracle_case(
            ConformanceDisposition::Passed,
            ConformanceStage::Complete,
            true,
            None,
        )];
        assert_eq!(
            ArtifactEnvelope::conformance(ConformanceArtifact {
                report_version: CONFORMANCE_REPORT_VERSION,
                identity: conformance_identity(),
                summary: computed_conformance_summary(&cases),
                cases,
            }),
            Err(ArtifactError::InvalidConformanceCase(
                "case oracle/disposition/provenance invariant"
            )),
            "the canonical constructor must reject a passed case without an oracle kind"
        );
    }

    #[test]
    fn conformance_errata_bind_erratum_passes_and_not_applicable_cases_to_the_identity() {
        let passed = |id: &str, erratum: Option<&str>| ConformanceCase {
            id: id.into(),
            assertion_id: id.into(),
            spec_num: "3.1".into(),
            spec_id: "#state".into(),
            datamodel: "xpath".into(),
            conformance: ConformanceLevel::Mandatory,
            manual: false,
            stage: ConformanceStage::Complete,
            disposition: ConformanceDisposition::Passed,
            reason_code: None,
            reason: None,
            diagnostics: Vec::new(),
            configuration: vec!["pass".into()],
            terminal_status: Some(ConformanceTerminalStatus::Finished),
            commands: 1,
            fuel_consumed: 1,
            oracle: Some(ConformanceOracleKind::OutcomeLog),
            erratum: erratum.map(str::to_owned),
        };
        let not_applicable = ConformanceCase {
            stage: ConformanceStage::Selection,
            disposition: ConformanceDisposition::NotApplicable,
            reason_code: Some(NOT_APPLICABLE_REASON_CODE.into()),
            reason: Some("Not applicable per W3C".into()),
            configuration: Vec::new(),
            terminal_status: None,
            commands: 0,
            fuel_consumed: 0,
            oracle: None,
            ..passed("302", None)
        };
        let cases = vec![
            passed("241", Some("xpath-241")),
            passed("250", None),
            not_applicable,
        ];
        let mut identity = conformance_identity();
        identity.suite.errata = Some(ConformanceErrataIdentity {
            source: "xpath.json".into(),
            digest: "f".repeat(64),
            applied: BTreeMap::from([("241".into(), "xpath-241".into())]),
            not_applicable: vec!["302".into()],
        });
        let summary = computed_conformance_summary(&cases);
        assert_eq!(summary.total_selected, 3);
        assert_eq!(summary.counts.applicable_selected, 2);
        assert_eq!(summary.counts.passed, 2);
        assert_eq!(summary.counts.passed_under_erratum, 1);
        assert_eq!(summary.counts.not_applicable, 1);
        assert_eq!(summary.counts.not_passing, 0);
        assert_eq!(summary.mandatory.not_applicable, 1);
        let artifact = ArtifactEnvelope::conformance(ConformanceArtifact {
            report_version: CONFORMANCE_REPORT_VERSION,
            identity: identity.clone(),
            summary: summary.clone(),
            cases: cases.clone(),
        })
        .expect("errata fixture must produce an artifact");
        let text = RustcRenderer::default()
            .render(&artifact)
            .expect("errata artifact must render as rustc text");
        assert!(text.contains("note[w3c-not-applicable]: IRP case 302 is not applicable"));
        assert!(
            text.contains("note[erratum]: IRP case 241 passed under documented erratum xpath-241")
        );
        assert!(text.contains("2/2 passed (1 under documented errata)"));
        assert!(text.contains("1 not applicable"));
        let reject = |cases: Vec<ConformanceCase>, identity: ConformanceRunIdentity| {
            ArtifactEnvelope::conformance(ConformanceArtifact {
                report_version: CONFORMANCE_REPORT_VERSION,
                identity,
                summary: computed_conformance_summary(&cases),
                cases,
            })
        };
        // An erratum id the identity does not record is rejected.
        let mut unknown = cases.clone();
        unknown[1].erratum = Some("xpath-unknown".into());
        assert!(matches!(
            reject(unknown, identity.clone()),
            Err(ArtifactError::InvalidConformanceCase(
                "case erratum/not-applicable/errata identity invariant"
            ))
        ));
        // A recorded erratum case must say so.
        let mut hidden = cases.clone();
        hidden[0].erratum = None;
        assert!(reject(hidden, identity.clone()).is_err());
        // A declared not-applicable case cannot be executed or passed.
        let mut executed = cases.clone();
        executed[2] = passed("302", None);
        assert!(reject(executed, identity.clone()).is_err());
        // No errata identity means no erratum and no not-applicable case.
        let mut bare = identity;
        bare.suite.errata = None;
        assert!(reject(cases, bare).is_err());
    }

    #[test]
    fn conformance_payload_round_trips_and_reports_authoritative_dispositions() {
        let cases = vec![
            ConformanceCase {
                id: "pass-1".into(),
                assertion_id: "a1".into(),
                spec_num: "3.1".into(),
                spec_id: "3.1".into(),
                datamodel: "null".into(),
                conformance: ConformanceLevel::Mandatory,
                manual: false,
                stage: ConformanceStage::Complete,
                disposition: ConformanceDisposition::Passed,
                reason_code: None,
                reason: None,
                diagnostics: Vec::new(),
                configuration: vec!["done".into()],
                terminal_status: Some(ConformanceTerminalStatus::Finished),
                commands: 1,
                fuel_consumed: 1,
                oracle: Some(ConformanceOracleKind::OutcomeLog),
                erratum: None,
            },
            ConformanceCase {
                id: "unsupported-2".into(),
                assertion_id: "a2".into(),
                spec_num: "4.1".into(),
                spec_id: "4.1".into(),
                datamodel: "null".into(),
                conformance: ConformanceLevel::Mandatory,
                manual: false,
                stage: ConformanceStage::Capability,
                disposition: ConformanceDisposition::Unsupported,
                reason_code: Some("cap001".into()),
                reason: Some("unsupported capability".into()),
                diagnostics: Vec::new(),
                configuration: Vec::new(),
                terminal_status: None,
                commands: 0,
                fuel_consumed: 0,
                oracle: None,
                erratum: None,
            },
        ];
        let summary = computed_conformance_summary(&cases);
        let artifact = ArtifactEnvelope::conformance(ConformanceArtifact {
            report_version: CONFORMANCE_REPORT_VERSION,
            identity: conformance_identity(),
            summary,
            cases,
        })
        .expect("conformance fixture must produce an artifact");
        let bytes = encode_canonical(&artifact).expect("conformance artifact must encode");
        assert_eq!(
            decode_canonical(&bytes).expect("conformance artifact bytes must decode"),
            artifact
        );
        let mut unsupported: Value =
            serde_json::from_slice(&bytes).expect("conformance bytes must be valid JSON");
        unsupported["conformance"]["report_version"] = json!(CONFORMANCE_REPORT_VERSION + 1);
        assert!(matches!(
            decode_canonical(
                &serde_json::to_vec(&unsupported).expect("unsupported-version JSON must serialize"),
            ),
            Err(ArtifactError::UnsupportedPayloadVersion {
                kind: CONFORMANCE_KIND,
                ..
            })
        ));
        let text = RustcRenderer::default()
            .render(&artifact)
            .expect("conformance artifact must render as rustc text");
        assert!(text.contains("error[cap001]"));
        assert!(text.contains("1/2 passed (0 under documented errata); 1 not passing"));
        let sarif_bytes =
            render_sarif(&artifact).expect("conformance artifact must render as SARIF");
        let sarif: Value =
            serde_json::from_slice(&sarif_bytes).expect("conformance SARIF must be valid JSON");
        assert_eq!(sarif["runs"][0]["results"][0]["ruleId"], "cap001");
        assert_eq!(
            sarif["runs"][0]["properties"]["summary"]["counts"]["not_passing"],
            1
        );
        assert_eq!(
            sarif["runs"][0]["properties"]["identity"]["suite"]["source_set_digest"],
            "b".repeat(64)
        );
        assert_eq!(
            artifact
                .conformance_payload()
                .expect("conformance payload")
                .identity
                .filters
                .ids,
            ["001", "002"]
        );
    }

    #[test]
    fn conformance_decoder_fails_closed_on_identity_tampering() {
        let cases = Vec::new();
        let artifact = ArtifactEnvelope::conformance(ConformanceArtifact {
            report_version: CONFORMANCE_REPORT_VERSION,
            identity: conformance_identity(),
            summary: computed_conformance_summary(&cases),
            cases,
        })
        .expect("identity fixture must be canonical");
        let bytes = encode_canonical(&artifact).expect("identity fixture must encode");

        let mut missing_source_set: Value =
            serde_json::from_slice(&bytes).expect("identity bytes must be JSON");
        missing_source_set["conformance"]["identity"]["suite"]
            .as_object_mut()
            .expect("suite must be an object")
            .remove("source_set_digest");
        assert!(matches!(
            decode_canonical(
                &serde_json::to_vec(&missing_source_set)
                    .expect("missing identity JSON must serialize")
            ),
            Err(ArtifactError::InvalidJson(_))
        ));

        let mut invalid_digest: Value =
            serde_json::from_slice(&bytes).expect("identity bytes must be JSON");
        invalid_digest["conformance"]["identity"]["suite"]["manifest_digest"] =
            json!("not-a-digest");
        assert_eq!(
            decode_canonical(
                &serde_json::to_vec(&invalid_digest).expect("invalid identity JSON must serialize")
            ),
            Err(ArtifactError::InvalidConformanceIdentity(
                "suite.manifest_digest"
            ))
        );

        let mut missing_revision: Value =
            serde_json::from_slice(&bytes).expect("identity bytes must be JSON");
        missing_revision["conformance"]["identity"]["suite"]["revision"] = Value::Null;
        assert_eq!(
            decode_canonical(
                &serde_json::to_vec(&missing_revision)
                    .expect("missing revision JSON must serialize")
            ),
            Err(ArtifactError::InvalidConformanceIdentity(
                "suite.official_irp_provenance"
            ))
        );
    }

    #[test]
    fn conformance_decoder_rejects_content_tampering_bound_by_report_digest() {
        let cases = vec![ConformanceCase {
            id: "2".into(),
            assertion_id: "a2".into(),
            spec_num: "3.1".into(),
            spec_id: "#state".into(),
            datamodel: "null".into(),
            conformance: ConformanceLevel::Mandatory,
            manual: false,
            stage: ConformanceStage::Complete,
            disposition: ConformanceDisposition::Passed,
            reason_code: None,
            reason: None,
            diagnostics: Vec::new(),
            configuration: vec!["done".into()],
            terminal_status: Some(ConformanceTerminalStatus::Finished),
            commands: 1,
            fuel_consumed: 1,
            oracle: Some(ConformanceOracleKind::OutcomeLog),
            erratum: None,
        }];
        let artifact = ArtifactEnvelope::conformance(ConformanceArtifact {
            report_version: CONFORMANCE_REPORT_VERSION,
            identity: conformance_identity(),
            summary: computed_conformance_summary(&cases),
            cases,
        })
        .expect("fixture currently accepts a shape-only digest");
        let mut tampered = artifact;
        let ArtifactPayload::Conformance { conformance } = &mut tampered.payload else {
            panic!("fixture must contain conformance");
        };
        conformance.cases[0].commands = 2;
        conformance.cases[0].fuel_consumed = 2;
        let tampered_bytes =
            serde_json::to_vec(&tampered).expect("tampered fixture must serialize canonically");
        assert_eq!(
            decode_canonical(&tampered_bytes),
            Err(ArtifactError::ConformanceDigestMismatch)
        );
    }

    #[test]
    fn conformance_constructor_preserves_numeric_case_order() {
        let case = |id: &str| ConformanceCase {
            id: id.into(),
            assertion_id: id.into(),
            spec_num: "3.1".into(),
            spec_id: "#state".into(),
            datamodel: "null".into(),
            conformance: ConformanceLevel::Mandatory,
            manual: false,
            stage: ConformanceStage::Complete,
            disposition: ConformanceDisposition::Passed,
            reason_code: None,
            reason: None,
            diagnostics: Vec::new(),
            configuration: vec!["done".into()],
            terminal_status: Some(ConformanceTerminalStatus::Finished),
            commands: 1,
            fuel_consumed: 1,
            oracle: Some(ConformanceOracleKind::OutcomeLog),
            erratum: None,
        };
        let cases = vec![case("2"), case("10")];
        let artifact = ArtifactEnvelope::conformance(ConformanceArtifact {
            report_version: CONFORMANCE_REPORT_VERSION,
            identity: conformance_identity(),
            summary: computed_conformance_summary(&cases),
            cases,
        })
        .expect("numeric producer order must be accepted");
        assert_eq!(
            artifact
                .conformance_payload()
                .expect("conformance payload")
                .cases
                .iter()
                .map(|case| case.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "10"]
        );
    }

    #[test]
    fn conformance_contract_accepts_a_rejected_document_only_without_execution() {
        let case = ConformanceCase {
            id: "2".into(),
            assertion_id: "a2".into(),
            spec_num: "3.1".into(),
            spec_id: "#state".into(),
            datamodel: "null".into(),
            conformance: ConformanceLevel::Mandatory,
            manual: false,
            stage: ConformanceStage::Complete,
            disposition: ConformanceDisposition::Passed,
            reason_code: None,
            reason: None,
            diagnostics: Vec::new(),
            configuration: vec!["done".into()],
            terminal_status: Some(ConformanceTerminalStatus::Finished),
            commands: 1,
            fuel_consumed: 1,
            oracle: Some(ConformanceOracleKind::OutcomeLog),
            erratum: None,
        };
        // A rejected document never ran: it is accepted only with no
        // configuration, commands, or fuel.
        let mut rejected = case;
        rejected.manual = true;
        rejected.oracle = Some(ConformanceOracleKind::ManualEquivalent);
        rejected.terminal_status = Some(ConformanceTerminalStatus::Rejected);
        rejected.configuration = Vec::new();
        rejected.commands = 0;
        rejected.fuel_consumed = 0;
        let rejected_cases = vec![rejected.clone()];
        ArtifactEnvelope::conformance(ConformanceArtifact {
            report_version: CONFORMANCE_REPORT_VERSION,
            identity: conformance_identity(),
            summary: computed_conformance_summary(&rejected_cases),
            cases: rejected_cases,
        })
        .expect("a rejected document with no execution is a valid passed case");
        for mutate in [
            (|case: &mut ConformanceCase| case.configuration = vec!["done".into()])
                as fn(&mut ConformanceCase),
            |case| case.commands = 1,
            |case| case.fuel_consumed = 1,
        ] {
            let mut ran = rejected.clone();
            mutate(&mut ran);
            let ran_cases = vec![ran];
            assert!(matches!(
                ArtifactEnvelope::conformance(ConformanceArtifact {
                    report_version: CONFORMANCE_REPORT_VERSION,
                    identity: conformance_identity(),
                    summary: computed_conformance_summary(&ran_cases),
                    cases: ran_cases,
                }),
                Err(ArtifactError::InvalidConformanceCase(
                    "case disposition/stage/execution invariant"
                ))
            ));
        }
    }

    #[test]
    fn conformance_contract_rejects_duplicates_unknown_vocabulary_and_impossible_states() {
        let case = ConformanceCase {
            id: "2".into(),
            assertion_id: "a2".into(),
            spec_num: "3.1".into(),
            spec_id: "#state".into(),
            datamodel: "null".into(),
            conformance: ConformanceLevel::Mandatory,
            manual: false,
            stage: ConformanceStage::Complete,
            disposition: ConformanceDisposition::Passed,
            reason_code: None,
            reason: None,
            diagnostics: Vec::new(),
            configuration: vec!["done".into()],
            terminal_status: Some(ConformanceTerminalStatus::Finished),
            commands: 1,
            fuel_consumed: 1,
            oracle: Some(ConformanceOracleKind::OutcomeLog),
            erratum: None,
        };
        let duplicate_cases = vec![case.clone(), case.clone()];
        assert!(matches!(
            ArtifactEnvelope::conformance(ConformanceArtifact {
                report_version: CONFORMANCE_REPORT_VERSION,
                identity: conformance_identity(),
                summary: computed_conformance_summary(&duplicate_cases),
                cases: duplicate_cases,
            }),
            Err(ArtifactError::InvalidConformanceCase("case.id duplicate"))
        ));

        let mut impossible = case.clone();
        impossible.disposition = ConformanceDisposition::Unsupported;
        impossible.reason_code = Some("unsupported-feature".into());
        impossible.reason = Some("not implemented".into());
        let impossible_cases = vec![impossible];
        assert!(matches!(
            ArtifactEnvelope::conformance(ConformanceArtifact {
                report_version: CONFORMANCE_REPORT_VERSION,
                identity: conformance_identity(),
                summary: computed_conformance_summary(&impossible_cases),
                cases: impossible_cases,
            }),
            Err(ArtifactError::InvalidConformanceCase(
                "case disposition/stage/execution invariant"
            ))
        ));

        let valid = ArtifactEnvelope::conformance(ConformanceArtifact {
            report_version: CONFORMANCE_REPORT_VERSION,
            identity: conformance_identity(),
            summary: computed_conformance_summary(std::slice::from_ref(&case)),
            cases: vec![case.clone()],
        })
        .expect("valid fixture");
        let mut unknown_stage = serde_json::to_value(valid).expect("fixture must serialize");
        unknown_stage["conformance"]["cases"][0]["stage"] = json!("future-stage");
        assert!(matches!(
            decode_canonical(
                &serde_json::to_vec(&unknown_stage).expect("unknown stage must serialize")
            ),
            Err(ArtifactError::InvalidJson(_))
        ));

        let mut unknown_severity = unknown_stage;
        unknown_severity["conformance"]["cases"][0]["stage"] = json!("complete");
        unknown_severity["conformance"]["cases"][0]["diagnostics"] = json!([{
            "code": "SCXML001",
            "severity": "critical",
            "message": "unknown severity must fail closed"
        }]);
        assert!(matches!(
            decode_canonical(
                &serde_json::to_vec(&unknown_severity).expect("unknown severity must serialize")
            ),
            Err(ArtifactError::InvalidJson(_))
        ));

        let mut duplicate_configuration = case;
        duplicate_configuration.configuration.push("done".into());
        let duplicate_configuration_cases = vec![duplicate_configuration];
        assert!(matches!(
            ArtifactEnvelope::conformance(ConformanceArtifact {
                report_version: CONFORMANCE_REPORT_VERSION,
                identity: conformance_identity(),
                summary: computed_conformance_summary(&duplicate_configuration_cases),
                cases: duplicate_configuration_cases,
            }),
            Err(ArtifactError::InvalidConformanceCase(
                "case.configuration duplicate"
            ))
        ));
    }
}
