use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use scxml_syntax::{Element, Node, SourceId, SourceText, parse_xml};
use scxml_telemetry::{
    ArtifactMeasurement, ErrorCategory, Module, Operation, SignalEvent, observe,
    record_artifact_measurement,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::report::TerminalStatus;

/// The source id of the IRP manifest document: every span points into it.
const MANIFEST_SOURCE: SourceId = SourceId::new(1);

/// Supported prepared-suite manifest schema version.
pub const MANIFEST_VERSION: u32 = 6;
/// Default maximum engine operations allowed for one case.
pub const DEFAULT_COMMAND_BUDGET: u64 = 1_024;
/// Canonical prepared-suite manifest file name.
pub const CORPUS_FILE: &str = "corpus.json";
/// Maximum accepted encoded size of a prepared suite manifest.
pub const MAX_SUITE_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
/// Maximum accepted encoded size of an official IRP XML manifest.
pub const MAX_IRP_MANIFEST_BYTES: usize = 2 * 1024 * 1024;
/// Maximum accepted size of one case source, dependency, or resource file.
pub const MAX_CASE_FILE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum accepted UTF-8 byte length of one manifest string or path.
pub const MAX_MANIFEST_STRING_BYTES: usize = 16 * 1024;
/// Maximum accepted transformer command arguments.
pub const MAX_TRANSFORMER_COMMAND_ARGS: usize = 64;
/// Maximum accepted number of cases or official IRP assertions.
pub const MAX_MANIFEST_ITEMS: usize = 4_096;
/// Maximum accepted number of entries in one nested manifest collection.
pub const MAX_NESTED_ITEMS: usize = 1_024;
/// Maximum accepted aggregate number of nested collection entries.
pub const MAX_TOTAL_NESTED_ITEMS: usize = 65_536;
/// Maximum accepted engine operations for one prepared case.
pub const MAX_COMMAND_BUDGET: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Provenance classification for a prepared corpus.
pub enum CorpusKind {
    /// Corpus derived from the official W3C implementation report plan.
    OfficialIrp,
    /// Synthetic fixtures used only to test harness behavior.
    SyntheticHarness,
    /// A repository namespace's own implementation report plan
    /// (`namespaces/<name>/irp`): assertions over the namespace's README in
    /// the W3C IRP manifest format, prepared by the W3C templates and
    /// decided by the same `Outcome` oracle (or, for a document the
    /// namespace requires to be rejected, the document-rejection oracle).
    /// Its passes are never W3C IRP passes.
    NamespaceIrp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Recorded identity of the transformer used during corpus preparation.
pub struct TransformerIdentity {
    /// Stable transformer implementation name.
    pub name: String,
    /// Captured transformer version identity.
    pub version: String,
    /// Portable transformer argument vector, excluding per-case paths:
    /// absolute install paths are reduced to file names and the host sandbox
    /// wrapper is not recorded, so the identity is independent of one
    /// machine's install layout.
    pub command_argv: Vec<String>,
    /// SHA-256 over the portable argument vector and the transformer
    /// artifact bytes (the Saxon JAR for `java -jar` launches, otherwise the
    /// transformer executable) when the artifact was resolvable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_digest: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Canonical manifest for an immutable prepared conformance suite.
pub struct SuiteManifest {
    /// Manifest schema version; must equal [`MANIFEST_VERSION`].
    pub manifest_version: u32,
    /// Corpus and preparation identity shared by all cases.
    pub suite: SuiteIdentity,
    #[serde(default = "default_command_budget")]
    /// Default per-case execution command budget.
    pub default_command_budget: u64,
    /// Prepared cases in the suite.
    pub cases: Vec<CaseManifest>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Identity and provenance that make suite runs comparable.
pub struct SuiteIdentity {
    /// Stable suite identifier.
    pub id: String,
    /// Suite release or snapshot version.
    pub version: String,
    /// Whether this is official IRP data or a synthetic harness fixture.
    pub kind: CorpusKind,
    /// Source manifest location or acquisition identifier.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional source revision pinned during preparation.
    pub revision: Option<String>,
    /// SHA-256 digest of the complete prepared corpus identity.
    pub corpus_digest: String,
    /// SHA-256 digest of the acquired source set.
    pub source_digest: String,
    /// SHA-256 digest of the source manifest bytes.
    pub manifest_digest: String,
    #[serde(default)]
    /// Template path to SHA-256 digest mapping.
    pub template_digests: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Transformer identity, when transformation was required.
    pub transformer: Option<TransformerIdentity>,
    /// Data model selected during corpus specialization.
    pub datamodel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional licensing or acquisition provenance statement.
    pub provenance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Identity of the checked-in IRP errata applied during preparation,
    /// absent when preparation applied none.
    pub errata: Option<ErrataIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Identity of one checked-in IRP errata file applied during preparation.
///
/// Invariants (checked by [`load_suite`] and [`write_suite`]): `digest` is a
/// SHA-256; `applied` maps each corrected case id to its erratum id and equals
/// the per-case [`CaseErratum`] records exactly; `not_applicable` is the
/// sorted, duplicate-free set of case ids carrying [`NotApplicable`], disjoint
/// from `applied`.
pub struct ErrataIdentity {
    /// Portable errata file name (the digest identifies its exact bytes).
    pub source: String,
    /// SHA-256 of the exact errata file bytes.
    pub digest: String,
    /// Corrected case id to erratum id.
    pub applied: BTreeMap<String, String>,
    /// Case ids the W3C declares not applicable to this data model.
    pub not_applicable: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Transformation input an erratum corrects.
pub enum ErratumStage {
    /// The data-model XSLT template (the upstream defect is in the template).
    Template,
    /// The upstream `.txml` source (the upstream defect is in the test).
    Source,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Provenance of one case whose executable artifact was corrected by a
/// documented erratum. The case's `source`/`output_digest` name the corrected
/// output; the exact upstream-produced output is retained beside it.
pub struct CaseErratum {
    /// Erratum identifier from the errata file.
    pub id: String,
    /// Which transformation input the erratum corrected.
    pub stage: ErratumStage,
    /// Relative path to the corrected transformation input.
    pub corrected_input: PathBuf,
    /// SHA-256 of the corrected transformation input.
    pub corrected_input_digest: String,
    /// Relative path to the unmodified upstream-produced output.
    pub upstream_output: PathBuf,
    /// SHA-256 of the unmodified upstream-produced output.
    pub upstream_output_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A case the W3C declares not applicable to the prepared data model; it is
/// excluded from the applicable denominator and never executed.
pub struct NotApplicable {
    /// The W3C instruction that makes the case not applicable, with evidence.
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Normative conformance level assigned by the official manifest.
pub enum ConformanceLevel {
    /// A required implementation-report case.
    Mandatory,
    /// An optional implementation-report case.
    Optional,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
/// Observable condition used to decide whether a case passes.
pub enum ExpectedOutcome {
    /// Requires the machine to finish with exactly one active final state.
    FinalState {
        /// Required final state identifier.
        state: String,
    },
    /// Requires an exact active-state set, independent of ordering.
    ActiveConfiguration {
        /// Required active state identifiers.
        states: Vec<String>,
    },
    /// Requires the engine to report a specific terminal status.
    TerminalStatus {
        /// Required terminal status.
        status: TerminalStatus,
    },
    /// Requires the exact official pass-only `Outcome` log protocol.
    W3cLogOutcome {
        /// Declared log value; official manifests require [`LogOutcome::Pass`].
        outcome: LogOutcome,
    },
    /// Requires the canonical compiler to reject the document at parse or
    /// validation with exactly these distinct error codes (namespace
    /// corpora only); nothing runs.
    DocumentRejection {
        /// The expected error codes, sorted and distinct.
        codes: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Declared value for the official W3C `Outcome` log oracle.
pub enum LogOutcome {
    /// The case must log exactly `pass`.
    Pass,
    /// Invalid for prepared suites; retained so untrusted manifests fail with a
    /// typed oracle-contract error after decoding.
    Fail,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A preparation-time gap retained as an explicit case disposition.
pub struct PreparationFailure {
    /// Stable machine-readable preparation reason code.
    pub code: String,
    /// Human-readable bounded failure description.
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Canonical metadata and artifact references for one prepared case.
pub struct CaseManifest {
    /// Stable case identifier.
    pub id: String,
    /// Identifier of the governing specification assertion.
    pub assertion_id: String,
    /// Human-readable specification section number.
    pub spec_num: String,
    /// Stable specification fragment identifier.
    pub spec_id: String,
    /// Normative conformance level.
    pub conformance: ConformanceLevel,
    /// Data model selected for this prepared case.
    pub datamodel: String,
    /// Original case start URI from the source manifest.
    pub start_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Relative path to the transformed executable SCXML artifact.
    pub source: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// SHA-256 digest of the acquired source artifact.
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Relative path to the acquired source artifact.
    pub source_artifact: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// SHA-256 digest of the transformed executable artifact.
    pub output_digest: Option<String>,
    #[serde(default)]
    /// Relative paths to transformed dependency artifacts.
    pub dependencies: Vec<PathBuf>,
    #[serde(default)]
    /// Transformed dependency path to SHA-256 digest mapping.
    pub dependency_digests: BTreeMap<String, String>,
    #[serde(default)]
    /// Acquired dependency path to SHA-256 digest mapping.
    pub dependency_source_digests: BTreeMap<String, String>,
    #[serde(default)]
    /// Acquired dependency URI to immutable source-artifact path mapping.
    pub dependency_source_artifacts: BTreeMap<String, PathBuf>,
    #[serde(default)]
    /// Relative paths to non-SCXML resources required by the case.
    pub resources: Vec<PathBuf>,
    #[serde(default)]
    /// Resource path to SHA-256 digest mapping.
    pub resource_digests: BTreeMap<String, String>,
    #[serde(default)]
    /// Stable case classification tags.
    pub tags: Vec<String>,
    #[serde(default)]
    /// Source assertion description.
    pub description: String,
    #[serde(default)]
    /// Declared feature identifiers used for selection or reporting.
    pub features: Vec<String>,
    #[serde(default)]
    /// Whether the official manifest marks this case as manual.
    pub manual: bool,
    #[serde(default)]
    /// Ordered external event names supplied during execution.
    pub events: Vec<String>,
    /// Observable pass oracle for the case.
    pub expected: ExpectedOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional preparation timeout recorded in milliseconds.
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional case-specific execution command budget.
    pub max_commands: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Explicit preparation gap that prevents execution.
    pub preparation_failure: Option<PreparationFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Documented erratum that corrected this case's executable artifact.
    pub erratum: Option<CaseErratum>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// W3C not-applicable declaration for this case's data model.
    pub not_applicable: Option<NotApplicable>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Validated suite manifest together with resolved case artifacts.
pub struct LoadedSuite {
    /// Canonical absolute path of the loaded manifest.
    pub manifest_path: PathBuf,
    /// Canonical corpus root used to resolve relative artifacts.
    pub root: PathBuf,
    /// Validated canonical manifest.
    pub manifest: SuiteManifest,
    /// Loaded cases in deterministic identifier order.
    pub cases: Vec<LoadedCase>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// One validated case with resolved local paths and source text.
pub struct LoadedCase {
    /// Canonical case metadata.
    pub manifest: CaseManifest,
    /// Resolved executable source path, when preparation succeeded.
    pub source_path: Option<PathBuf>,
    /// UTF-8 executable SCXML source, when preparation succeeded.
    pub source: Option<String>,
    /// Resolved, integrity-checked dependency paths.
    pub dependency_paths: Vec<PathBuf>,
    /// Acquired dependency URI to integrity-checked source bytes (bounded).
    pub dependency_sources: Vec<(String, Vec<u8>)>,
    /// Resolved resource paths.
    pub resource_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Parsed official W3C implementation report plan manifest.
pub struct IrpManifest {
    /// Base URI used to resolve test and dependency references.
    pub base_uri: String,
    /// Specification assertions in source order.
    pub assertions: Vec<IrpAssertion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// One specification assertion and its official tests.
pub struct IrpAssertion {
    /// Stable assertion identifier.
    pub id: String,
    /// Human-readable specification section number.
    pub spec_num: String,
    /// Stable specification fragment identifier.
    pub spec_id: String,
    /// Assertion description from the official manifest.
    pub description: String,
    /// Tests associated with the assertion.
    pub tests: Vec<IrpTest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// One official test declaration before expansion by start URI.
pub struct IrpTest {
    /// Stable test identifier.
    pub id: String,
    /// Normative conformance level.
    pub conformance: ConformanceLevel,
    /// Whether the test requires manual execution.
    pub manual: bool,
    /// Start-document URIs that become executable cases.
    pub starts: Vec<String>,
    /// Dependency URIs shared by the test's start documents.
    pub dependencies: Vec<String>,
    /// Error codes the canonical compiler must reject the start documents
    /// with (`<reject code="..."/>` children, sorted and distinct); empty
    /// for a test decided by the `Outcome` oracle. The W3C manifest has
    /// none; only namespace manifests may declare them.
    pub rejects: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Flattened official case produced from one test start URI.
pub struct IrpCase {
    /// Stable, suite-unique case identifier.
    pub id: String,
    /// Governing assertion identifier.
    pub assertion_id: String,
    /// Human-readable specification section number.
    pub spec_num: String,
    /// Stable specification fragment identifier.
    pub spec_id: String,
    /// Governing assertion description.
    pub description: String,
    /// Normative conformance level.
    pub conformance: ConformanceLevel,
    /// Whether the source manifest marks the case as manual.
    pub manual: bool,
    /// Source start-document URI.
    pub start_uri: String,
    /// Source dependency URIs.
    pub dependencies: Vec<String>,
    /// Error codes the document must be rejected with (see
    /// [`IrpTest::rejects`]).
    pub rejects: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// A bounded manifest parsing, validation, integrity, or I/O failure.
pub struct ManifestError {
    /// Stable machine-readable failure code.
    pub code: &'static str,
    /// Human-readable failure description.
    pub message: String,
}

struct BoundedRead<'a> {
    path: &'a Path,
    limit: usize,
    size_error_code: &'static str,
}

struct TextBound<'a> {
    label: &'a str,
    value: &'a str,
    allow_empty: bool,
}

struct CollectionBound<'a> {
    label: &'a str,
    count: usize,
    limit: usize,
}

impl ManifestError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ManifestError {}

/// Parses the W3C IRP assertion manifest.
///
/// `fallback_base_uri` must come from the trusted acquisition boundary and is
/// used only when the historical live manifest omits its `baseuri` attribute.
///
/// # Errors
///
/// Returns a bounded manifest error for invalid UTF-8/XML, missing required
/// metadata, an unavailable base URI, an unknown element or attribute, or an
/// unsupported field value. Recognized IRP elements and attributes must be
/// unqualified; namespace-qualified lookalikes are rejected.
pub fn parse_irp_manifest(
    source_name: &str,
    fallback_base_uri: Option<&str>,
    bytes: &[u8],
) -> Result<IrpManifest, ManifestError> {
    validate_irp_inputs(source_name, fallback_base_uri, bytes)?;
    let text = std::str::from_utf8(bytes).map_err(|error| {
        ManifestError::new(
            "irp-manifest-utf8",
            format!("manifest is not UTF-8: {error}"),
        )
    })?;
    let document = parse_xml(SourceText::new(MANIFEST_SOURCE, source_name, text))
        .map_err(|error| ManifestError::new("irp-manifest-xml", error.to_string()))?;
    if !document.root.name.is(None, "assertions") {
        return Err(ManifestError::new(
            "irp-manifest-root",
            "official manifest root must be the unqualified `assertions` element",
        ));
    }
    validate_irp_element(&document.root, "assertions", &["baseuri"], &["assert"])?;
    let base_uri = irp_base_uri(&document.root, fallback_base_uri)?;
    let mut assertions = Vec::new();
    let mut total_nested_items = 0_usize;
    for assertion in child_elements(&document.root, "assert") {
        if assertions.len() == MAX_MANIFEST_ITEMS {
            return Err(ManifestError::new(
                "irp-manifest-count",
                "official manifest exceeds the assertion limit",
            ));
        }
        assertions.push(parse_irp_assertion(assertion, &mut total_nested_items)?);
    }
    if assertions.is_empty() {
        return Err(ManifestError::new(
            "irp-manifest-empty",
            "official manifest contains no assertions",
        ));
    }
    Ok(IrpManifest {
        base_uri,
        assertions,
    })
}

fn validate_irp_inputs(
    source_name: &str,
    fallback_base_uri: Option<&str>,
    bytes: &[u8],
) -> Result<(), ManifestError> {
    if bytes.len() > MAX_IRP_MANIFEST_BYTES {
        return Err(ManifestError::new(
            "irp-manifest-size",
            format!(
                "official manifest is {} bytes; limit is {MAX_IRP_MANIFEST_BYTES} bytes",
                bytes.len()
            ),
        ));
    }
    if source_name.len() > MAX_MANIFEST_STRING_BYTES {
        return Err(ManifestError::new(
            "irp-manifest-string",
            "manifest source name exceeds the configured string limit",
        ));
    }
    if fallback_base_uri.is_some_and(|value| value.len() > MAX_MANIFEST_STRING_BYTES) {
        return Err(ManifestError::new(
            "irp-manifest-string",
            "trusted acquisition base URI exceeds the configured string limit",
        ));
    }
    Ok(())
}

fn irp_base_uri(root: &Element, fallback_base_uri: Option<&str>) -> Result<String, ManifestError> {
    let base_uri = root
        .attribute("baseuri")
        .map(|attribute| attribute.value.as_str())
        .filter(|value| !value.is_empty())
        .or(fallback_base_uri.filter(|value| !value.is_empty()))
        .ok_or_else(|| {
            ManifestError::new(
                "irp-base-uri",
                "official manifest requires a non-empty baseuri attribute or trusted acquisition base URI",
            )
        })?
        .to_owned();
    if base_uri.len() > MAX_MANIFEST_STRING_BYTES {
        return Err(ManifestError::new(
            "irp-manifest-string",
            "official manifest base URI exceeds the configured string limit",
        ));
    }
    Ok(base_uri)
}

fn parse_irp_assertion(
    assertion: &Element,
    total_nested_items: &mut usize,
) -> Result<IrpAssertion, ManifestError> {
    validate_irp_element(assertion, "assert", &["id", "specnum", "specid"], &["test"])?;
    let id = required_attribute(assertion, "id")?.to_owned();
    let spec_num = required_attribute(assertion, "specnum")?.to_owned();
    let spec_id = required_attribute(assertion, "specid")?.to_owned();
    let description = direct_text(assertion)?.trim().to_owned();
    let mut tests = Vec::new();
    for test in child_elements(assertion, "test") {
        if tests.len() == MAX_NESTED_ITEMS {
            return Err(ManifestError::new(
                "irp-manifest-count",
                format!("assertion `{id}` exceeds the test limit"),
            ));
        }
        add_nested_items(total_nested_items, 1)?;
        tests.push(parse_irp_test(test, total_nested_items)?);
    }
    Ok(IrpAssertion {
        id,
        spec_num,
        spec_id,
        description,
        tests,
    })
}

fn parse_irp_test(
    test: &Element,
    total_nested_items: &mut usize,
) -> Result<IrpTest, ManifestError> {
    validate_irp_element(
        test,
        "test",
        &["id", "conformance", "manual"],
        &["start", "dep", "reject"],
    )?;
    let test_id = required_attribute(test, "id")?.to_owned();
    let conformance = parse_conformance(required_attribute(test, "conformance")?)?;
    let manual = parse_bool_attribute(test, "manual")?;
    let mut starts = Vec::new();
    for start in child_elements(test, "start") {
        validate_irp_element(start, "start", &["uri"], &[])?;
        if starts.len() == MAX_NESTED_ITEMS {
            return Err(ManifestError::new(
                "irp-manifest-count",
                format!("test `{test_id}` exceeds the start limit"),
            ));
        }
        starts.push(required_attribute(start, "uri")?.to_owned());
    }
    if starts.is_empty() {
        return Err(ManifestError::new(
            "irp-test-start",
            format!("test `{test_id}` has no start element"),
        ));
    }
    let mut dependencies = Vec::new();
    for dependency in child_elements(test, "dep") {
        validate_irp_element(dependency, "dep", &["uri"], &[])?;
        if dependencies.len() == MAX_NESTED_ITEMS {
            return Err(ManifestError::new(
                "irp-manifest-count",
                format!("test `{test_id}` exceeds the dependency limit"),
            ));
        }
        dependencies.push(required_attribute(dependency, "uri")?.to_owned());
    }
    let mut rejects = BTreeSet::new();
    for reject in child_elements(test, "reject") {
        validate_irp_element(reject, "reject", &["code"], &[])?;
        if rejects.len() == MAX_NESTED_ITEMS {
            return Err(ManifestError::new(
                "irp-manifest-count",
                format!("test `{test_id}` exceeds the reject limit"),
            ));
        }
        let code = required_attribute(reject, "code")?;
        if code.is_empty()
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(ManifestError::new(
                "irp-test-reject",
                format!("test `{test_id}` names a reject code that is not a diagnostic code"),
            ));
        }
        rejects.insert(code.to_owned());
    }
    add_nested_items(total_nested_items, starts.len())?;
    add_nested_items(total_nested_items, dependencies.len())?;
    add_nested_items(total_nested_items, rejects.len())?;
    Ok(IrpTest {
        id: test_id,
        conformance,
        manual,
        starts,
        dependencies,
        rejects: rejects.into_iter().collect(),
    })
}

fn add_nested_items(total: &mut usize, count: usize) -> Result<(), ManifestError> {
    *total = total.checked_add(count).ok_or_else(|| {
        ManifestError::new("irp-manifest-count", "official manifest count overflow")
    })?;
    if *total > MAX_TOTAL_NESTED_ITEMS {
        return Err(ManifestError::new(
            "irp-manifest-count",
            "official manifest exceeds the aggregate collection limit",
        ));
    }
    Ok(())
}

/// Expands every official test start URI into a deterministic bounded case list.
///
/// # Errors
///
/// Returns [`ManifestError`] when expansion exceeds [`MAX_MANIFEST_ITEMS`] or
/// a generated suffix conflicts with an existing natural or generated id.
pub fn flatten_irp_manifest(manifest: &IrpManifest) -> Result<Vec<IrpCase>, ManifestError> {
    let mut cases = Vec::new();
    let mut ids = BTreeSet::new();
    let mut next_suffix = BTreeMap::<String, u32>::new();
    for assertion in &manifest.assertions {
        for test in &assertion.tests {
            for start in &test.starts {
                if cases.len() == MAX_MANIFEST_ITEMS {
                    return Err(ManifestError::new(
                        "irp-case-count",
                        "flattened official manifest exceeds the case limit",
                    ));
                }
                let stem = Path::new(start)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or(&test.id)
                    .strip_prefix("test")
                    .unwrap_or(&test.id);
                let preferred = if test.starts.len() == 1 {
                    test.id.clone()
                } else {
                    stem.to_owned()
                };
                let id = allocate_case_id(&preferred, &mut ids, &mut next_suffix)?;
                cases.push(IrpCase {
                    id,
                    assertion_id: assertion.id.clone(),
                    spec_num: assertion.spec_num.clone(),
                    spec_id: assertion.spec_id.clone(),
                    description: assertion.description.clone(),
                    conformance: test.conformance,
                    manual: test.manual,
                    start_uri: start.clone(),
                    dependencies: test.dependencies.clone(),
                    rejects: test.rejects.clone(),
                });
            }
        }
    }
    cases.sort_by(|left, right| case_id_sort_key(&left.id).cmp(&case_id_sort_key(&right.id)));
    Ok(cases)
}

fn allocate_case_id(
    preferred: &str,
    ids: &mut BTreeSet<String>,
    next_suffix: &mut BTreeMap<String, u32>,
) -> Result<String, ManifestError> {
    if ids.insert(preferred.to_owned()) {
        next_suffix.entry(preferred.to_owned()).or_insert(2);
        return Ok(preferred.to_owned());
    }
    let suffix = next_suffix.entry(preferred.to_owned()).or_insert(2);
    let candidate = format!("{preferred}-{suffix}");
    *suffix = suffix
        .checked_add(1)
        .ok_or_else(|| ManifestError::new("irp-case-id", "generated case id suffix overflow"))?;
    if ids.insert(candidate.clone()) {
        return Ok(candidate);
    }
    Err(ManifestError::new(
        "irp-case-id",
        "generated case id conflicts with an existing manifest id",
    ))
}

/// Loads and integrity-checks a prepared suite directory or manifest file.
///
/// # Errors
///
/// Returns [`ManifestError`] for unavailable or oversized files, non-canonical
/// or unsupported manifest data, unsafe relative paths, invalid UTF-8 source,
/// missing artifacts, digest mismatches, or violated collection bounds.
pub fn load_suite(path: &Path) -> Result<LoadedSuite, ManifestError> {
    let span = observe(Module::ConformanceManifest, Operation::ManifestLoadSuite);
    let loaded = (|| {
        let requested = if path.is_dir() {
            path.join(CORPUS_FILE)
        } else {
            path.to_path_buf()
        };
        let manifest_path = requested.canonicalize().map_err(|error| {
            ManifestError::new(
                "manifest-read",
                format!("cannot resolve manifest `{}`: {error}", requested.display()),
            )
        })?;
        let root = manifest_path
            .parent()
            .ok_or_else(|| ManifestError::new("manifest-root", "manifest has no parent directory"))?
            .to_path_buf();
        let bytes = read_suite_manifest(&manifest_path)?;
        let mut manifest: SuiteManifest = serde_json::from_slice(&bytes).map_err(|error| {
            ManifestError::new(
                "manifest-json",
                format!("cannot parse `{}`: {error}", manifest_path.display()),
            )
        })?;
        validate_manifest(&manifest)?;
        manifest
            .cases
            .sort_by(|left, right| case_id_sort_key(&left.id).cmp(&case_id_sort_key(&right.id)));

        let mut cases = Vec::with_capacity(manifest.cases.len());
        for case in &manifest.cases {
            cases.push(load_case(&root, case)?);
        }

        verify_suite_identity(&manifest)?;
        verify_bound_errata(&root, &manifest)?;
        let manifest_bytes = u64::try_from(bytes.len()).map_err(|_| {
            ManifestError::new("identity-size", "manifest byte count is unsupported")
        })?;
        let case_count = u64::try_from(cases.len()).map_err(|_| {
            ManifestError::new("identity-size", "manifest case count is unsupported")
        })?;
        record_artifact_measurement(ArtifactMeasurement::SuiteManifestBytes, manifest_bytes);
        record_artifact_measurement(ArtifactMeasurement::SuiteCases, case_count);
        span.event(SignalEvent::ArtifactAccepted);

        Ok(LoadedSuite {
            manifest_path,
            root,
            manifest,
            cases,
        })
    })();
    if loaded.is_err() {
        span.fail(ErrorCategory::InputRead);
    }
    loaded
}

fn load_case(root: &Path, case: &CaseManifest) -> Result<LoadedCase, ManifestError> {
    if let Some(relative) = &case.source_artifact {
        let path = resolve_case_path(root, &case.id, "source-artifact", relative)?;
        verify_digest(
            &path,
            case.source_digest.as_deref(),
            &case.id,
            "source-artifact",
        )?;
    }
    if let Some(erratum) = &case.erratum {
        for (kind, relative, digest) in [
            (
                "erratum-corrected-input",
                &erratum.corrected_input,
                &erratum.corrected_input_digest,
            ),
            (
                "erratum-upstream-output",
                &erratum.upstream_output,
                &erratum.upstream_output_digest,
            ),
        ] {
            let path = resolve_case_path(root, &case.id, kind, relative)?;
            verify_digest(&path, Some(digest.as_str()), &case.id, kind)?;
        }
    }
    let mut dependency_sources = Vec::new();
    for (source_uri, relative) in &case.dependency_source_artifacts {
        let path = resolve_case_path(root, &case.id, "dependency-source-artifact", relative)?;
        verify_digest(
            &path,
            case.dependency_source_digests
                .get(source_uri)
                .map(String::as_str),
            &case.id,
            "dependency-source-artifact",
        )?;
        let bytes = read_case_file(&path)?;
        dependency_sources.push((source_uri.clone(), bytes));
    }
    let (source_path, source) = if let Some(relative) = &case.source {
        let path = resolve_case_path(root, &case.id, "source", relative)?;
        verify_digest(&path, case.output_digest.as_deref(), &case.id, "source")?;
        let source = String::from_utf8(read_case_file(&path)?).map_err(|error| {
            ManifestError::new(
                "case-source-read",
                format!(
                    "case `{}` cannot read source `{}` as UTF-8: {error}",
                    case.id,
                    path.display()
                ),
            )
        })?;
        (Some(path), Some(source))
    } else {
        (None, None)
    };
    let dependency_paths = resolve_paths(root, &case.id, "dependency", &case.dependencies)?;
    verify_case_digests(
        root,
        &case.id,
        "dependency",
        &dependency_paths,
        &case.dependency_digests,
    )?;
    // Transformed dependency documents are executable sources for `<data
    // src="file:...">` initializers and `<invoke src="file:...">` child
    // machines (SCXML 5.3/6.4). They ride behind the same integrity checks
    // as every other prepared artifact.
    for path in &dependency_paths {
        // Transformed dependency documents are executable XML sources; a
        // non-UTF-8 artifact cannot be a valid document and must fail closed
        // instead of being silently dropped from the executable set.
        let text = String::from_utf8(read_case_file(path)?).map_err(|error| {
            ManifestError::new(
                "case-dependency-utf8",
                format!(
                    "case `{}` dependency `{}` is not valid UTF-8: {error}",
                    case.id,
                    path.display()
                ),
            )
        })?;
        let relative = path.strip_prefix(root).unwrap_or(path);
        dependency_sources.push((slash_path(relative), text.into_bytes()));
    }
    let resource_paths = resolve_paths(root, &case.id, "resource", &case.resources)?;
    verify_case_digests(
        root,
        &case.id,
        "resource",
        &resource_paths,
        &case.resource_digests,
    )?;
    Ok(LoadedCase {
        manifest: case.clone(),
        source_path,
        source,
        dependency_paths,
        dependency_sources,
        resource_paths,
    })
}

fn verify_case_digests(
    root: &Path,
    case_id: &str,
    kind: &'static str,
    paths: &[PathBuf],
    digests: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    for path in paths {
        let relative = path.strip_prefix(root).map_err(|_| {
            ManifestError::new(
                "case-path-escape",
                format!("resolved {kind} escaped corpus"),
            )
        })?;
        let key = slash_path(relative);
        verify_digest(path, digests.get(&key).map(String::as_str), case_id, kind)?;
    }
    Ok(())
}

/// Validates and writes a suite manifest as canonical pretty JSON.
///
/// # Errors
///
/// Returns [`ManifestError`] when the manifest violates schema, version,
/// identity, path, digest, string, or collection invariants; serialization
/// fails; or the destination cannot be written.
pub fn write_suite(path: &Path, manifest: &SuiteManifest) -> Result<(), ManifestError> {
    let span = observe(Module::ConformanceManifest, Operation::ManifestWriteSuite);
    let result = (|| {
        validate_manifest(manifest)?;
        verify_suite_identity(manifest)?;
        let mut bytes = serde_json::to_vec_pretty(manifest).map_err(|error| {
            ManifestError::new("manifest-json", format!("cannot encode corpus: {error}"))
        })?;
        bytes.push(b'\n');
        fs::write(path, bytes).map_err(|error| {
            ManifestError::new(
                "manifest-write",
                format!("cannot write `{}`: {error}", path.display()),
            )
        })
    })();
    if result.is_err() {
        span.fail(ErrorCategory::Report);
    }
    result
}

#[must_use]
/// Computes a lowercase SHA-256 digest for in-memory bytes.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

struct DigestWriter(Sha256);

impl Write for DigestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Computes the canonical acquired source-set identity from case metadata.
///
/// Paths are sorted and length-delimited before hashing. Repeated source names
/// must carry the same digest; conflicting identities fail closed.
///
/// # Errors
///
/// Returns [`ManifestError`] for incomplete or conflicting source metadata.
pub fn source_set_digest(cases: &[CaseManifest]) -> Result<String, ManifestError> {
    let mut identities = BTreeMap::<&str, &str>::new();
    for case in cases {
        match (&case.source_artifact, &case.source_digest) {
            (Some(_), Some(digest)) => {
                insert_source_identity(&mut identities, &case.start_uri, digest, &case.id)?;
            }
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                return Err(ManifestError::new(
                    "source-identity-incomplete",
                    format!(
                        "case `{}` source artifact and digest must be present together",
                        case.id
                    ),
                ));
            }
        }
        for (path, digest) in &case.dependency_source_digests {
            insert_source_identity(&mut identities, path, digest, &case.id)?;
        }
    }
    let mut digest = Sha256::new();
    digest.update(b"scxml.source-set/v1\0");
    for (path, value) in identities {
        update_length_delimited(&mut digest, path.as_bytes())?;
        update_length_delimited(&mut digest, value.as_bytes())?;
    }
    Ok(hex::encode(digest.finalize()))
}

fn insert_source_identity<'a>(
    identities: &mut BTreeMap<&'a str, &'a str>,
    path: &'a str,
    digest: &'a str,
    case_id: &str,
) -> Result<(), ManifestError> {
    if let Some(previous) = identities.insert(path, digest)
        && previous != digest
    {
        return Err(ManifestError::new(
            "source-identity-conflict",
            format!("case `{case_id}` conflicts with another digest for source `{path}`"),
        ));
    }
    Ok(())
}

fn update_length_delimited(digest: &mut Sha256, bytes: &[u8]) -> Result<(), ManifestError> {
    let length = u64::try_from(bytes.len())
        .map_err(|_| ManifestError::new("identity-size", "identity field length is unsupported"))?;
    digest.update(length.to_le_bytes());
    digest.update(bytes);
    Ok(())
}

/// Computes the canonical prepared-suite identity over all normalized metadata.
///
/// The returned digest binds suite provenance, source-set identity, templates,
/// transformer, command budgets, filters/tags/features, oracles, failures, and all
/// referenced artifact digest maps. The `corpus_digest` value itself is cleared
/// before streaming canonical JSON into SHA-256.
///
/// # Errors
///
/// Returns [`ManifestError`] when canonical serialization fails.
pub fn suite_identity_digest(manifest: &SuiteManifest) -> Result<String, ManifestError> {
    let mut canonical = manifest.clone();
    canonical.suite.corpus_digest.clear();
    canonical
        .cases
        .sort_by(|left, right| case_id_sort_key(&left.id).cmp(&case_id_sort_key(&right.id)));
    let mut writer = DigestWriter(Sha256::new());
    serde_json::to_writer(&mut writer, &canonical).map_err(|error| {
        ManifestError::new(
            "suite-identity-encode",
            format!("cannot encode canonical suite identity: {error}"),
        )
    })?;
    Ok(hex::encode(writer.0.finalize()))
}

fn verify_suite_identity(manifest: &SuiteManifest) -> Result<(), ManifestError> {
    let source_digest = source_set_digest(&manifest.cases)?;
    if source_digest != manifest.suite.source_digest {
        return Err(ManifestError::new(
            "source-identity-tampered",
            "recomputed acquired source-set digest does not match suite metadata",
        ));
    }
    let corpus_digest = suite_identity_digest(manifest)?;
    if corpus_digest != manifest.suite.corpus_digest {
        return Err(ManifestError::new(
            "suite-identity-tampered",
            "recomputed prepared-suite digest does not match suite metadata",
        ));
    }
    Ok(())
}

/// Computes a lowercase SHA-256 digest for one bounded regular file.
///
/// # Errors
///
/// Returns [`ManifestError`] when `path` cannot be inspected or read, is not a
/// regular file, exceeds [`MAX_CASE_FILE_BYTES`], or the platform cannot
/// represent the configured size bound.
pub fn sha256_file(path: &Path) -> Result<String, ManifestError> {
    let span = observe(Module::ConformanceManifest, Operation::ManifestSha256File);
    let result = (|| {
        let (file, length) = open_regular_file(path, "digest-read")?;
        let limit = u64::try_from(MAX_CASE_FILE_BYTES).map_err(|_| {
            ManifestError::new("case-file-size", "case file byte limit is unsupported")
        })?;
        if length.len() > limit {
            return Err(ManifestError::new(
                "case-file-size",
                format!(
                    "file `{}` is {} bytes; limit is {MAX_CASE_FILE_BYTES} bytes",
                    path.display(),
                    length.len()
                ),
            ));
        }
        let mut reader = BufReader::new(file).take(limit.saturating_add(1));
        let mut digest = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            let read = reader.read(&mut buffer).map_err(|error| {
                ManifestError::new(
                    "digest-read",
                    format!("cannot hash `{}`: {error}", path.display()),
                )
            })?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(u64::try_from(read).map_err(|_| {
                    ManifestError::new("digest-read", "hash read length is unsupported")
                })?)
                .ok_or_else(|| ManifestError::new("digest-read", "hash byte count overflow"))?;
            if copied > limit {
                break;
            }
            digest.update(&buffer[..read]);
        }
        if copied > limit {
            return Err(ManifestError::new(
                "case-file-size",
                format!(
                    "file `{}` exceeds the {MAX_CASE_FILE_BYTES}-byte limit",
                    path.display()
                ),
            ));
        }
        Ok(hex::encode(digest.finalize()))
    })();
    if result.is_err() {
        span.fail(ErrorCategory::InputRead);
    }
    result
}

fn read_suite_manifest(path: &Path) -> Result<Vec<u8>, ManifestError> {
    read_bounded_file(&BoundedRead {
        path,
        limit: MAX_SUITE_MANIFEST_BYTES,
        size_error_code: "manifest-size",
    })
}

fn read_case_file(path: &Path) -> Result<Vec<u8>, ManifestError> {
    read_bounded_file(&BoundedRead {
        path,
        limit: MAX_CASE_FILE_BYTES,
        size_error_code: "case-file-size",
    })
}

/// Opens `path` only when it is a regular file: the path is checked before
/// it is opened (opening a FIFO or a device can block) and the handle again
/// after (the path may have changed in between).
fn open_regular_file(
    path: &Path,
    code: &'static str,
) -> Result<(fs::File, fs::Metadata), ManifestError> {
    let not_regular =
        || ManifestError::new(code, format!("`{}` is not a regular file", path.display()));
    let inspect = |error: std::io::Error| {
        ManifestError::new(
            code,
            format!("cannot inspect `{}`: {error}", path.display()),
        )
    };
    if !fs::metadata(path).map_err(inspect)?.is_file() {
        return Err(not_regular());
    }
    let file = fs::File::open(path).map_err(|error| {
        ManifestError::new(code, format!("cannot open `{}`: {error}", path.display()))
    })?;
    let metadata = file.metadata().map_err(inspect)?;
    if !metadata.is_file() {
        return Err(not_regular());
    }
    Ok((file, metadata))
}

fn read_bounded_file(request: &BoundedRead<'_>) -> Result<Vec<u8>, ManifestError> {
    let (file, length) = open_regular_file(request.path, "manifest-read")?;
    let limit_u64 = u64::try_from(request.limit).map_err(|_| {
        ManifestError::new(
            request.size_error_code,
            "configured byte limit is unsupported",
        )
    })?;
    if length.len() > limit_u64 {
        return Err(ManifestError::new(
            request.size_error_code,
            format!(
                "file `{}` is {} bytes; limit is {} bytes",
                request.path.display(),
                length.len(),
                request.limit
            ),
        ));
    }
    let capacity = usize::try_from(length.len()).map_err(|_| {
        ManifestError::new(request.size_error_code, "file length cannot fit in memory")
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    BufReader::new(file)
        .take(limit_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ManifestError::new(
                "manifest-read",
                format!("cannot read `{}`: {error}", request.path.display()),
            )
        })?;
    if bytes.len() > request.limit {
        return Err(ManifestError::new(
            request.size_error_code,
            format!(
                "file `{}` exceeds the {}-byte limit",
                request.path.display(),
                request.limit
            ),
        ));
    }
    Ok(bytes)
}

#[allow(
    clippy::too_many_lines,
    reason = "one fail-closed pass validates every manifest field, bound, and oracle rule in order"
)]
fn validate_manifest(manifest: &SuiteManifest) -> Result<(), ManifestError> {
    if manifest.manifest_version != MANIFEST_VERSION {
        return Err(ManifestError::new(
            "manifest-version",
            format!(
                "unsupported manifest version {}; expected {MANIFEST_VERSION}",
                manifest.manifest_version
            ),
        ));
    }
    require_collection_bound(&CollectionBound {
        label: "suite cases",
        count: manifest.cases.len(),
        limit: MAX_MANIFEST_ITEMS,
    })?;
    require_non_empty("suite id", &manifest.suite.id)?;
    require_non_empty("suite version", &manifest.suite.version)?;
    require_non_empty("suite source", &manifest.suite.source)?;
    require_digest("suite corpus_digest", &manifest.suite.corpus_digest)?;
    require_digest("suite source_digest", &manifest.suite.source_digest)?;
    require_digest("suite manifest_digest", &manifest.suite.manifest_digest)?;
    require_non_empty("suite datamodel", &manifest.suite.datamodel)?;
    if let Some(revision) = &manifest.suite.revision {
        require_non_empty("suite revision", revision)?;
    }
    if let Some(provenance) = &manifest.suite.provenance {
        require_bounded_text(&TextBound {
            label: "suite provenance",
            value: provenance,
            allow_empty: true,
        })?;
    }
    require_collection_bound(&CollectionBound {
        label: "suite template digests",
        count: manifest.suite.template_digests.len(),
        limit: MAX_NESTED_ITEMS,
    })?;
    for (name, digest) in &manifest.suite.template_digests {
        require_non_empty("template name", name)?;
        require_digest("template digest", digest)?;
    }
    if let Some(transformer) = &manifest.suite.transformer {
        require_non_empty("transformer name", &transformer.name)?;
        require_non_empty("transformer version", &transformer.version)?;
        require_collection_bound(&CollectionBound {
            label: "transformer command arguments",
            count: transformer.command_argv.len(),
            limit: MAX_TRANSFORMER_COMMAND_ARGS,
        })?;
        if transformer.command_argv.is_empty() {
            return Err(ManifestError::new(
                "transformer-command",
                "transformer command argument vector must not be empty",
            ));
        }
        for argument in &transformer.command_argv {
            require_non_empty("transformer command argument", argument)?;
        }
        if let Some(digest) = &transformer.executable_digest {
            require_digest("transformer command-artifact digest", digest)?;
        }
    }
    if matches!(
        manifest.suite.kind,
        CorpusKind::OfficialIrp | CorpusKind::NamespaceIrp
    ) && (manifest.suite.transformer.is_none()
        || manifest.suite.template_digests.is_empty()
        || manifest.suite.revision.is_none())
    {
        return Err(ManifestError::new(
            "official-corpus-identity",
            "official IRP corpus requires revision, transformer, and template identities",
        ));
    }
    if !(1..=MAX_COMMAND_BUDGET).contains(&manifest.default_command_budget) {
        return Err(ManifestError::new(
            "manifest-command-budget",
            format!("default_command_budget must be between 1 and {MAX_COMMAND_BUDGET}"),
        ));
    }
    let mut ids = BTreeSet::new();
    let mut total_nested_items = manifest.suite.template_digests.len();
    for case in &manifest.cases {
        require_non_empty("case id", &case.id)?;
        if !ids.insert(case.id.clone()) {
            return Err(ManifestError::new(
                "duplicate-case-id",
                format!("duplicate case id `{}`", case.id),
            ));
        }
        require_non_empty(
            &format!("case `{}` assertion_id", case.id),
            &case.assertion_id,
        )?;
        require_non_empty(&format!("case `{}` spec_num", case.id), &case.spec_num)?;
        require_non_empty(&format!("case `{}` spec_id", case.id), &case.spec_id)?;
        require_non_empty(&format!("case `{}` datamodel", case.id), &case.datamodel)?;
        require_non_empty(&format!("case `{}` start_uri", case.id), &case.start_uri)?;
        for (label, value) in [
            ("case description", case.description.as_str()),
            ("case assertion id", case.assertion_id.as_str()),
            ("case spec number", case.spec_num.as_str()),
            ("case spec id", case.spec_id.as_str()),
            ("case datamodel", case.datamodel.as_str()),
            ("case start URI", case.start_uri.as_str()),
        ] {
            require_bounded_text(&TextBound {
                label,
                value,
                allow_empty: label == "case description",
            })?;
        }
        for (label, path) in [
            ("case source", case.source.as_ref()),
            ("case source artifact", case.source_artifact.as_ref()),
        ] {
            if let Some(path) = path {
                require_bounded_text(&TextBound {
                    label,
                    value: path.as_os_str().to_str().ok_or_else(|| {
                        ManifestError::new(
                            "manifest-path-utf8",
                            format!("{label} must be valid UTF-8"),
                        )
                    })?,
                    allow_empty: false,
                })?;
            }
        }
        let collections = [
            ("case dependencies", case.dependencies.len()),
            ("case dependency digests", case.dependency_digests.len()),
            (
                "case dependency source digests",
                case.dependency_source_digests.len(),
            ),
            (
                "case dependency source artifacts",
                case.dependency_source_artifacts.len(),
            ),
            ("case resources", case.resources.len()),
            ("case resource digests", case.resource_digests.len()),
            ("case tags", case.tags.len()),
            ("case features", case.features.len()),
            ("case events", case.events.len()),
        ];
        for (label, count) in collections {
            require_collection_bound(&CollectionBound {
                label,
                count,
                limit: MAX_NESTED_ITEMS,
            })?;
            total_nested_items = total_nested_items.checked_add(count).ok_or_else(|| {
                ManifestError::new("manifest-count", "manifest collection count overflow")
            })?;
        }
        if total_nested_items > MAX_TOTAL_NESTED_ITEMS {
            return Err(ManifestError::new(
                "manifest-count",
                "manifest exceeds the aggregate collection limit",
            ));
        }
        for path in case.dependencies.iter().chain(&case.resources) {
            require_bounded_text(&TextBound {
                label: "case relative path",
                value: path.as_os_str().to_str().ok_or_else(|| {
                    ManifestError::new(
                        "manifest-path-utf8",
                        "case relative path must be valid UTF-8",
                    )
                })?,
                allow_empty: false,
            })?;
        }
        for path in case.dependency_source_artifacts.values() {
            require_bounded_text(&TextBound {
                label: "dependency source artifact path",
                value: path.as_os_str().to_str().ok_or_else(|| {
                    ManifestError::new(
                        "manifest-path-utf8",
                        "dependency source artifact path must be valid UTF-8",
                    )
                })?,
                allow_empty: false,
            })?;
        }
        for value in case.tags.iter().chain(&case.features).chain(&case.events) {
            require_bounded_text(&TextBound {
                label: "case collection string",
                value,
                allow_empty: false,
            })?;
        }
        for (key, digest) in &case.dependency_digests {
            require_non_empty("dependency digest path", key)?;
            require_digest("dependency digest", digest)?;
        }
        for (key, digest) in &case.dependency_source_digests {
            require_non_empty("dependency source digest path", key)?;
            require_digest("dependency source digest", digest)?;
        }
        if case
            .dependency_source_artifacts
            .keys()
            .ne(case.dependency_source_digests.keys())
        {
            return Err(ManifestError::new(
                "dependency-source-identity-set",
                format!(
                    "case `{}` dependency source artifact and digest keys must match exactly",
                    case.id
                ),
            ));
        }
        for (key, digest) in &case.resource_digests {
            require_non_empty("resource digest path", key)?;
            require_digest("resource digest", digest)?;
        }
        require_exact_digest_keys(
            &case.id,
            "dependency",
            &case.dependencies,
            &case.dependency_digests,
        )?;
        require_exact_digest_keys(
            &case.id,
            "resource",
            &case.resources,
            &case.resource_digests,
        )?;
        if case.preparation_failure.is_none() && !case.manual && case.source.is_none() {
            return Err(ManifestError::new(
                "case-source-empty",
                format!("case `{}` requires a prepared source", case.id),
            ));
        }
        if case.source.is_some() && case.output_digest.is_none() {
            return Err(ManifestError::new(
                "case-output-digest",
                format!("case `{}` prepared source requires output_digest", case.id),
            ));
        }
        if let Some(digest) = &case.output_digest {
            require_digest("case output digest", digest)?;
        }
        if let Some(digest) = &case.source_digest {
            require_digest("case source digest", digest)?;
            if case.source_artifact.is_none() {
                return Err(ManifestError::new(
                    "case-source-artifact",
                    format!("case `{}` source digest requires source_artifact", case.id),
                ));
            }
        }
        if case
            .max_commands
            .is_some_and(|steps| !(1..=MAX_COMMAND_BUDGET).contains(&steps))
        {
            return Err(ManifestError::new(
                "case-command-budget",
                format!(
                    "case `{}` max_commands must be between 1 and {MAX_COMMAND_BUDGET}",
                    case.id
                ),
            ));
        }
        for event in &case.events {
            if event.is_empty() || event.chars().any(char::is_whitespace) {
                return Err(ManifestError::new(
                    "case-event-name",
                    format!(
                        "case `{}` external event names must be non-empty and contain no whitespace",
                        case.id
                    ),
                ));
            }
        }
        if let ExpectedOutcome::ActiveConfiguration { states } = &case.expected
            && (states.is_empty() || states.iter().any(String::is_empty))
        {
            return Err(ManifestError::new(
                "case-oracle",
                format!(
                    "case `{}` active configuration must contain non-empty state ids",
                    case.id
                ),
            ));
        }
        match &case.expected {
            ExpectedOutcome::FinalState { state } => {
                require_non_empty("expected final state", state)?;
            }
            ExpectedOutcome::ActiveConfiguration { states } => {
                require_collection_bound(&CollectionBound {
                    label: "expected active configuration",
                    count: states.len(),
                    limit: MAX_NESTED_ITEMS,
                })?;
                total_nested_items =
                    total_nested_items
                        .checked_add(states.len())
                        .ok_or_else(|| {
                            ManifestError::new(
                                "manifest-count",
                                "manifest collection count overflow",
                            )
                        })?;
                if total_nested_items > MAX_TOTAL_NESTED_ITEMS {
                    return Err(ManifestError::new(
                        "manifest-count",
                        "manifest exceeds the aggregate collection limit",
                    ));
                }
                for state in states {
                    require_non_empty("expected active state", state)?;
                }
            }
            ExpectedOutcome::DocumentRejection { codes } => {
                require_collection_bound(&CollectionBound {
                    label: "expected rejection codes",
                    count: codes.len(),
                    limit: MAX_NESTED_ITEMS,
                })?;
                if codes.is_empty() || !codes.windows(2).all(|pair| pair[0] < pair[1]) {
                    return Err(ManifestError::new(
                        "case-rejection-codes",
                        format!(
                            "case `{}` rejection codes must be non-empty, sorted, and distinct",
                            case.id
                        ),
                    ));
                }
                for code in codes {
                    require_non_empty("expected rejection code", code)?;
                }
            }
            ExpectedOutcome::TerminalStatus { .. } | ExpectedOutcome::W3cLogOutcome { .. } => {}
        }
        match (&manifest.suite.kind, &case.expected) {
            (
                CorpusKind::OfficialIrp | CorpusKind::NamespaceIrp,
                ExpectedOutcome::W3cLogOutcome {
                    outcome: LogOutcome::Pass,
                },
            )
            | (CorpusKind::NamespaceIrp, ExpectedOutcome::DocumentRejection { .. }) => {}
            (CorpusKind::OfficialIrp, _) => {
                return Err(ManifestError::new(
                    "official-case-oracle",
                    format!(
                        "official case `{}` must require exactly one Outcome log whose evaluated value is pass",
                        case.id
                    ),
                ));
            }
            (CorpusKind::SyntheticHarness, ExpectedOutcome::W3cLogOutcome { .. }) => {
                return Err(ManifestError::new(
                    "synthetic-case-oracle",
                    format!(
                        "synthetic case `{}` cannot claim the official IRP log oracle",
                        case.id
                    ),
                ));
            }
            (CorpusKind::SyntheticHarness, ExpectedOutcome::DocumentRejection { .. }) => {
                return Err(ManifestError::new(
                    "synthetic-case-oracle",
                    format!(
                        "synthetic case `{}` cannot claim the namespace document-rejection oracle",
                        case.id
                    ),
                ));
            }
            (CorpusKind::SyntheticHarness, _) => {}
            (CorpusKind::NamespaceIrp, _) => {
                return Err(ManifestError::new(
                    "namespace-case-oracle",
                    format!(
                        "namespace case `{}` must require the pass Outcome log or a document rejection",
                        case.id
                    ),
                ));
            }
        }
        if let Some(failure) = &case.preparation_failure {
            require_non_empty("preparation failure code", &failure.code)?;
            require_non_empty("preparation failure message", &failure.message)?;
        }
        validate_case_errata(manifest, case)?;
    }
    validate_errata_identity(manifest)?;
    Ok(())
}

/// Checks one case's erratum and not-applicable records against the suite's
/// errata identity.
fn validate_case_errata(
    manifest: &SuiteManifest,
    case: &CaseManifest,
) -> Result<(), ManifestError> {
    let identity = manifest.suite.errata.as_ref();
    if let Some(erratum) = &case.erratum {
        require_bounded_text(&TextBound {
            label: "case erratum id",
            value: &erratum.id,
            allow_empty: false,
        })?;
        require_digest(
            "case erratum corrected input digest",
            &erratum.corrected_input_digest,
        )?;
        require_digest(
            "case erratum upstream output digest",
            &erratum.upstream_output_digest,
        )?;
        for (label, path) in [
            ("case erratum corrected input", &erratum.corrected_input),
            ("case erratum upstream output", &erratum.upstream_output),
        ] {
            require_bounded_text(&TextBound {
                label,
                value: path.as_os_str().to_str().ok_or_else(|| {
                    ManifestError::new("manifest-path-utf8", format!("{label} must be valid UTF-8"))
                })?,
                allow_empty: false,
            })?;
        }
        let recorded = identity.and_then(|errata| errata.applied.get(&case.id));
        if recorded != Some(&erratum.id)
            || case.source.is_none()
            || case.preparation_failure.is_some()
            || case.not_applicable.is_some()
            || manifest.suite.kind != CorpusKind::OfficialIrp
        {
            return Err(ManifestError::new(
                "case-erratum",
                format!(
                    "case `{}` erratum must be recorded in the official suite errata identity, correct a prepared source, and exclude preparation failures and not-applicable declarations",
                    case.id
                ),
            ));
        }
    } else if identity.is_some_and(|errata| errata.applied.contains_key(&case.id)) {
        return Err(ManifestError::new(
            "case-erratum",
            format!(
                "suite errata identity names case `{}` but the case records no erratum",
                case.id
            ),
        ));
    }
    let listed = identity.is_some_and(|errata| errata.not_applicable.contains(&case.id));
    match &case.not_applicable {
        Some(declaration) => {
            require_bounded_text(&TextBound {
                label: "case not-applicable reason",
                value: &declaration.reason,
                allow_empty: false,
            })?;
            if !listed || manifest.suite.kind != CorpusKind::OfficialIrp {
                return Err(ManifestError::new(
                    "case-not-applicable",
                    format!(
                        "case `{}` not-applicable declaration is missing from the official suite errata identity",
                        case.id
                    ),
                ));
            }
        }
        None if listed => {
            return Err(ManifestError::new(
                "case-not-applicable",
                format!(
                    "suite errata identity declares case `{}` not applicable but the case does not",
                    case.id
                ),
            ));
        }
        None => {}
    }
    Ok(())
}

/// Resolves the errata file a prepared corpus binds: exactly one plain file
/// name beside the manifest (never the manifest itself), resolving to a
/// regular file inside the corpus directory.
fn bound_errata_path(root: &Path, source: &str) -> Result<PathBuf, ManifestError> {
    let mut components = Path::new(source).components();
    let plain = matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(name)), None) if name == source
    );
    if !plain || source == CORPUS_FILE {
        return Err(ManifestError::new(
            "suite-errata-source",
            "suite errata source must be one file name beside the corpus manifest",
        ));
    }
    let path = root.join(source).canonicalize().map_err(|error| {
        ManifestError::new(
            "suite-errata-read",
            format!("the corpus errata file `{source}` is unavailable: {error}"),
        )
    })?;
    if !path.starts_with(root) {
        return Err(ManifestError::new(
            "suite-errata-source",
            format!("the corpus errata file `{source}` escapes the corpus directory"),
        ));
    }
    Ok(path)
}

/// Checks that a prepared corpus's errata declarations are exactly those
/// of the errata bytes it binds: the file beside the manifest must hash to
/// the declared digest, govern the suite's data model and template, and
/// produce exactly the suite errata identity and every case's erratum and
/// not-applicable record. A corpus whose declarations were edited, even
/// with a recomputed corpus identity, therefore fails closed.
fn verify_bound_errata(root: &Path, manifest: &SuiteManifest) -> Result<(), ManifestError> {
    let Some(identity) = &manifest.suite.errata else {
        return Ok(());
    };
    let path = bound_errata_path(root, &identity.source)?;
    let loaded = crate::errata::read_errata(&path).map_err(|error| {
        let code = if error.code == "errata-read" {
            "suite-errata-read"
        } else {
            "suite-errata-invalid"
        };
        ManifestError::new(
            code,
            format!("the corpus errata file is not valid errata: {error}"),
        )
    })?;
    if loaded.digest != identity.digest {
        return Err(ManifestError::new(
            "suite-errata-digest",
            "the corpus errata file does not hash to the declared errata digest",
        ));
    }
    if loaded.file.datamodel != manifest.suite.datamodel
        || manifest
            .suite
            .template_digests
            .get(&manifest.suite.datamodel)
            != Some(&loaded.file.template_sha256)
    {
        return Err(ManifestError::new(
            "suite-errata-binding",
            "the corpus errata govern another data model or template",
        ));
    }
    let derived = loaded.identity();
    if derived.applied != identity.applied || derived.not_applicable != identity.not_applicable {
        return Err(ManifestError::new(
            "suite-errata-identity",
            "the suite errata identity differs from the corrections and not-applicable declarations of the bound errata",
        ));
    }
    for case in &manifest.cases {
        let erratum_matches = match (loaded.erratum_for(&case.id), &case.erratum) {
            (None, None) => true,
            (Some(erratum), Some(recorded)) => {
                recorded.id == erratum.id
                    && recorded.stage == erratum.stage
                    && recorded.upstream_output_digest == erratum.upstream_output_sha256
                    && case.output_digest.as_deref()
                        == Some(erratum.corrected_output_sha256.as_str())
            }
            _ => false,
        };
        if !erratum_matches || case.not_applicable != loaded.not_applicable_record(&case.id) {
            return Err(ManifestError::new(
                "suite-errata-case",
                format!(
                    "case `{}` records an erratum or not-applicable declaration that the bound errata do not",
                    case.id
                ),
            ));
        }
    }
    Ok(())
}

/// Checks the suite errata identity's own fields and that every id it names
/// is a distinct case of the suite.
fn validate_errata_identity(manifest: &SuiteManifest) -> Result<(), ManifestError> {
    let Some(errata) = &manifest.suite.errata else {
        return Ok(());
    };
    require_bounded_text(&TextBound {
        label: "suite errata source",
        value: &errata.source,
        allow_empty: false,
    })?;
    require_digest("suite errata digest", &errata.digest)?;
    require_collection_bound(&CollectionBound {
        label: "suite errata applied",
        count: errata.applied.len(),
        limit: MAX_NESTED_ITEMS,
    })?;
    require_collection_bound(&CollectionBound {
        label: "suite errata not applicable",
        count: errata.not_applicable.len(),
        limit: MAX_NESTED_ITEMS,
    })?;
    if !errata
        .not_applicable
        .windows(2)
        .all(|pair| case_id_sort_key(&pair[0]) < case_id_sort_key(&pair[1]))
    {
        return Err(ManifestError::new(
            "suite-errata-order",
            "suite errata not_applicable must use canonical case order without duplicates",
        ));
    }
    let case_ids: BTreeSet<&str> = manifest.cases.iter().map(|case| case.id.as_str()).collect();
    let mut erratum_ids = BTreeSet::new();
    for (case_id, erratum_id) in &errata.applied {
        require_bounded_text(&TextBound {
            label: "suite errata erratum id",
            value: erratum_id,
            allow_empty: false,
        })?;
        if !case_ids.contains(case_id.as_str()) || !erratum_ids.insert(erratum_id.as_str()) {
            return Err(ManifestError::new(
                "suite-errata-unknown-case",
                format!(
                    "suite errata erratum `{erratum_id}` names an unknown case `{case_id}` or repeats an erratum id"
                ),
            ));
        }
    }
    for case_id in &errata.not_applicable {
        if !case_ids.contains(case_id.as_str()) || errata.applied.contains_key(case_id) {
            return Err(ManifestError::new(
                "suite-errata-unknown-case",
                format!(
                    "suite errata not-applicable case `{case_id}` is unknown or also corrected"
                ),
            ));
        }
    }
    Ok(())
}

fn require_exact_digest_keys(
    case_id: &str,
    kind: &str,
    paths: &[PathBuf],
    digests: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    if paths.len() != digests.len()
        || paths
            .iter()
            .any(|path| !digests.contains_key(&slash_path(path)))
    {
        return Err(ManifestError::new(
            "artifact-digest-set",
            format!("case `{case_id}` {kind} paths and digest keys must match exactly"),
        ));
    }
    Ok(())
}

fn require_non_empty(label: &str, value: &str) -> Result<(), ManifestError> {
    require_bounded_text(&TextBound {
        label,
        value,
        allow_empty: false,
    })
}

fn require_bounded_text(bound: &TextBound<'_>) -> Result<(), ManifestError> {
    if !bound.allow_empty && bound.value.is_empty() {
        Err(ManifestError::new(
            "manifest-required-field",
            format!("{} must not be empty", bound.label),
        ))
    } else if bound.value.len() > MAX_MANIFEST_STRING_BYTES {
        Err(ManifestError::new(
            "manifest-string-size",
            format!(
                "{} is {} bytes; limit is {MAX_MANIFEST_STRING_BYTES} bytes",
                bound.label,
                bound.value.len()
            ),
        ))
    } else {
        Ok(())
    }
}

fn require_collection_bound(bound: &CollectionBound<'_>) -> Result<(), ManifestError> {
    if bound.count > bound.limit {
        Err(ManifestError::new(
            "manifest-count",
            format!(
                "{} has {} entries; limit is {}",
                bound.label, bound.count, bound.limit
            ),
        ))
    } else {
        Ok(())
    }
}

fn require_digest(label: &str, value: &str) -> Result<(), ManifestError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ManifestError::new(
            "manifest-digest",
            format!("{label} must be a 64-character hexadecimal SHA-256 digest"),
        ))
    }
}

fn resolve_paths(
    root: &Path,
    case_id: &str,
    kind: &str,
    paths: &[PathBuf],
) -> Result<Vec<PathBuf>, ManifestError> {
    paths
        .iter()
        .map(|path| resolve_case_path(root, case_id, kind, path))
        .collect()
}

fn resolve_case_path(
    root: &Path,
    case_id: &str,
    kind: &str,
    relative: &Path,
) -> Result<PathBuf, ManifestError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
    {
        return Err(ManifestError::new(
            "case-path-absolute",
            format!(
                "case `{case_id}` {kind} path `{}` must be relative to the manifest",
                relative.display()
            ),
        ));
    }
    let canonical = root.join(relative).canonicalize().map_err(|error| {
        ManifestError::new(
            "case-path-missing",
            format!(
                "case `{case_id}` {kind} path `{}` is unavailable: {error}",
                relative.display()
            ),
        )
    })?;
    if !canonical.starts_with(root) {
        return Err(ManifestError::new(
            "case-path-escape",
            format!(
                "case `{case_id}` {kind} path `{}` escapes corpus directory `{}`",
                relative.display(),
                root.display()
            ),
        ));
    }
    if !canonical.is_file() {
        return Err(ManifestError::new(
            "case-path-not-file",
            format!(
                "case `{case_id}` {kind} path `{}` is not a file",
                relative.display()
            ),
        ));
    }
    Ok(canonical)
}

fn verify_digest(
    path: &Path,
    expected: Option<&str>,
    case_id: &str,
    kind: &str,
) -> Result<(), ManifestError> {
    let expected = expected.ok_or_else(|| {
        ManifestError::new(
            "artifact-digest-missing",
            format!("case `{case_id}` {kind} has no recorded digest"),
        )
    })?;
    let actual = sha256_file(path)?;
    if actual != expected {
        return Err(ManifestError::new(
            "artifact-tampered",
            format!(
                "case `{case_id}` {kind} `{}` digest does not match corpus metadata",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn child_elements<'a>(
    element: &'a Element,
    local_name: &'a str,
) -> impl Iterator<Item = &'a Element> {
    element
        .children
        .iter()
        .filter_map(move |child| match child {
            Node::Element(child) if child.name.is(None, local_name) => Some(child),
            Node::Element(_) | Node::Text(_) => None,
        })
}

fn validate_irp_element(
    element: &Element,
    expected_local_name: &str,
    allowed_attributes: &[&str],
    allowed_children: &[&str],
) -> Result<(), ManifestError> {
    if !element.name.is(None, expected_local_name) {
        return Err(ManifestError::new(
            "irp-unknown-element",
            format!(
                "expected unqualified `{expected_local_name}` but found `{}`",
                bounded_expanded_name(element)
            ),
        ));
    }
    for attribute in &element.attributes {
        let known = attribute.name.namespace_uri.is_none()
            && allowed_attributes.contains(&attribute.name.local_name.as_str());
        if !known {
            return Err(ManifestError::new(
                "irp-unknown-attribute",
                format!(
                    "unqualified `{expected_local_name}` contains unsupported attribute `{}`",
                    bounded_name_parts(
                        attribute.name.namespace_uri.as_deref(),
                        &attribute.name.local_name,
                    )
                ),
            ));
        }
    }
    for child in &element.children {
        let Node::Element(child) = child else {
            continue;
        };
        let known = child.name.namespace_uri.is_none()
            && allowed_children.contains(&child.name.local_name.as_str());
        if !known {
            return Err(ManifestError::new(
                "irp-unknown-element",
                format!(
                    "unqualified `{expected_local_name}` contains unsupported child element `{}`",
                    bounded_expanded_name(child)
                ),
            ));
        }
    }
    Ok(())
}

fn bounded_expanded_name(element: &Element) -> String {
    bounded_name_parts(
        element.name.namespace_uri.as_deref(),
        &element.name.local_name,
    )
}

fn bounded_name_parts(namespace_uri: Option<&str>, local_name: &str) -> String {
    const NAME_PART_LIMIT: usize = 128;
    let local_name = local_name.chars().take(NAME_PART_LIMIT).collect::<String>();
    match namespace_uri {
        Some(namespace_uri) => {
            let namespace_uri = namespace_uri
                .chars()
                .take(NAME_PART_LIMIT)
                .collect::<String>();
            format!("{{{namespace_uri}}}{local_name}")
        }
        None => local_name,
    }
}

fn direct_text(element: &Element) -> Result<String, ManifestError> {
    let mut value = String::new();
    for text in element.children.iter().filter_map(|child| match child {
        Node::Text(text) => Some(text.value.as_str()),
        Node::Element(_) => None,
    }) {
        let next_length = value.len().checked_add(text.len()).ok_or_else(|| {
            ManifestError::new("irp-manifest-string", "assertion text length overflow")
        })?;
        if next_length > MAX_MANIFEST_STRING_BYTES {
            return Err(ManifestError::new(
                "irp-manifest-string",
                "assertion description exceeds the configured string limit",
            ));
        }
        value.push_str(text);
    }
    Ok(value)
}

fn required_attribute<'a>(element: &'a Element, name: &str) -> Result<&'a str, ManifestError> {
    element
        .attribute(name)
        .map(|attribute| attribute.value.as_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ManifestError::new(
                "irp-required-attribute",
                format!("{} requires non-empty `{name}`", element.name.local_name),
            )
        })
        .and_then(|value| {
            if value.len() > MAX_MANIFEST_STRING_BYTES {
                Err(ManifestError::new(
                    "irp-manifest-string",
                    format!(
                        "{} `{name}` exceeds the configured string limit",
                        element.name.local_name
                    ),
                ))
            } else {
                Ok(value)
            }
        })
}

fn parse_bool_attribute(element: &Element, name: &str) -> Result<bool, ManifestError> {
    match required_attribute(element, name)? {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        value => Err(ManifestError::new(
            "irp-boolean-attribute",
            format!(
                "{} `{name}` has invalid boolean `{value}`",
                element.name.local_name
            ),
        )),
    }
}

fn parse_conformance(value: &str) -> Result<ConformanceLevel, ManifestError> {
    match value {
        "mandatory" => Ok(ConformanceLevel::Mandatory),
        "optional" => Ok(ConformanceLevel::Optional),
        _ => Err(ManifestError::new(
            "irp-conformance",
            format!("unknown conformance level `{value}`"),
        )),
    }
}

fn case_id_sort_key(id: &str) -> (u64, &str) {
    let digits = id.bytes().take_while(u8::is_ascii_digit).count();
    let numeric = id[..digits].parse().unwrap_or(u64::MAX);
    (numeric, id)
}

fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn default_command_budget() -> u64 {
    DEFAULT_COMMAND_BUDGET
}
