use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::Read as _;
use std::path::Path;

use scxml_telemetry::{
    ArtifactMeasurement, ErrorCategory, Module, Operation, observe, record_artifact_measurement,
};
use serde::{Deserialize, Serialize};

use crate::filter::Filters;
use crate::manifest::{ConformanceLevel, SuiteIdentity};

/// Version of the runner implementation encoded in produced reports.
pub const RUNNER_VERSION: &str = env!("CARGO_PKG_VERSION");

const MAX_REPORT_STRING_BYTES: usize = 64 * 1024;
const MAX_REPORT_COLLECTION_ITEMS: usize = 100_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Furthest pipeline stage reached by a case result.
pub enum Stage {
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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Final conformance disposition of one selected case.
pub enum Disposition {
    /// Execution satisfied the declared oracle.
    Passed,
    /// Supported execution completed but did not satisfy the oracle.
    Failed,
    /// The implementation cannot yet execute a required semantic capability.
    Unsupported,
    /// The case input was structurally or semantically invalid.
    Invalid,
    /// Harness preparation, integrity, or infrastructure failed.
    HarnessError,
    /// The case was selected but excluded by explicit policy.
    Skipped,
    /// The W3C declares the case not applicable to the prepared data model
    /// (recorded in the checked-in errata identity); it is excluded from the
    /// applicable denominator and never executed.
    NotApplicable,
}

/// Stable reason code of every [`Disposition::NotApplicable`] case.
pub const NOT_APPLICABLE_REASON_CODE: &str = "w3c-not-applicable";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Oracle that decided a case whose execution reached oracle evaluation.
pub enum OracleKind {
    /// The official W3C protocol: exactly one `Outcome` log whose value is
    /// exactly `pass`, in a root session finished in exactly the `pass`
    /// final state.
    OutcomeLog,
    /// The documented automated equivalent of a W3C manual instruction
    /// ([`crate::manual_oracle::ManualOracle`]), for official manual cases only.
    ManualEquivalent,
    /// A non-official synthetic-harness expectation.
    SyntheticExpectation,
    /// A namespace suite's rejection oracle: the canonical compiler rejected
    /// the document with exactly the error codes the suite manifest lists
    /// (`crate::oracle::evaluate_document_rejection`); nothing ran.
    DocumentRejection,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Observable terminal status of a machine execution.
pub enum TerminalStatus {
    /// The machine reached a stable configuration but can accept more events.
    Running,
    /// The machine reached its top-level final state.
    Finished,
    /// The processor rejected the document at load (SCXML 1.0 section 5.8:
    /// an external `<script src>` that cannot be fetched or authenticated);
    /// no session ran, so no configuration, command, or fuel exists.
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Bounded diagnostic copied from canonical validation output.
pub struct DiagnosticRecord {
    /// Stable diagnostic code.
    pub code: String,
    /// Canonical severity name.
    pub severity: String,
    /// Human-readable diagnostic message.
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Canonical observations and disposition for one selected case.
pub struct CaseResult {
    /// Stable case identifier.
    pub id: String,
    /// Governing assertion identifier.
    pub assertion_id: String,
    /// Human-readable specification section number.
    pub spec_num: String,
    /// Stable specification fragment identifier.
    pub spec_id: String,
    /// Data model selected for execution.
    pub datamodel: String,
    /// Normative conformance level.
    pub conformance: ConformanceLevel,
    /// Whether the source manifest marks the case as manual.
    pub manual: bool,
    /// Furthest pipeline stage reached.
    pub stage: Stage,
    /// Final conformance disposition.
    pub disposition: Disposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional stable reason code for a non-passing disposition.
    pub reason_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional human-readable reason for a non-passing disposition.
    pub reason: Option<String>,
    #[serde(default)]
    /// Ordered canonical diagnostics produced before disposition.
    pub diagnostics: Vec<DiagnosticRecord>,
    #[serde(default)]
    /// Active configuration observed after execution.
    pub configuration: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Terminal execution status, when execution began.
    pub terminal_status: Option<TerminalStatus>,
    /// Engine commands charged to the case.
    pub commands: u64,
    /// Deterministic execution fuel charged to the case.
    pub fuel_consumed: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Oracle that decided the case; present exactly when execution reached
    /// oracle evaluation (`Passed` at `Complete`, `Failed` at `Oracle`).
    pub oracle: Option<OracleKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Documented erratum that corrected the executed official artifact;
    /// equals the suite errata identity's entry for this case.
    pub erratum: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Aggregate disposition counts for one report partition.
pub struct Counts {
    /// Selected non-skipped cases in this partition.
    pub applicable_selected: u64,
    /// Cases with [`Disposition::Passed`].
    pub passed: u64,
    /// Cases with [`Disposition::Failed`].
    pub failed: u64,
    /// Cases with [`Disposition::Unsupported`].
    pub unsupported: u64,
    /// Cases with [`Disposition::Invalid`].
    pub invalid: u64,
    /// Cases with [`Disposition::HarnessError`].
    pub harness_error: u64,
    /// Cases with [`Disposition::Skipped`].
    pub skipped: u64,
    /// Cases with [`Disposition::NotApplicable`] (outside the denominator).
    pub not_applicable: u64,
    /// Sum of failed, unsupported, invalid, and harness-error cases.
    pub not_passing: u64,
    /// Passed cases whose executed artifact was corrected by an erratum
    /// (a subset of `passed`).
    pub passed_under_erratum: u64,
}

impl Counts {
    fn add(&mut self, case: &CaseResult) {
        match case.disposition {
            Disposition::Passed => {
                self.applicable_selected += 1;
                self.passed += 1;
                if case.erratum.is_some() {
                    self.passed_under_erratum += 1;
                }
            }
            Disposition::Failed => {
                self.applicable_selected += 1;
                self.failed += 1;
                self.not_passing += 1;
            }
            Disposition::Unsupported => {
                self.applicable_selected += 1;
                self.unsupported += 1;
                self.not_passing += 1;
            }
            Disposition::Invalid => {
                self.applicable_selected += 1;
                self.invalid += 1;
                self.not_passing += 1;
            }
            Disposition::HarnessError => {
                self.applicable_selected += 1;
                self.harness_error += 1;
                self.not_passing += 1;
            }
            Disposition::Skipped => self.skipped += 1,
            Disposition::NotApplicable => self.not_applicable += 1,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Aggregate counts for a complete run and its stable partitions.
pub struct Summary {
    /// Number of selected cases, including skipped cases.
    pub total_selected: u64,
    /// Number of selected non-skipped cases.
    pub applicable_selected: u64,
    /// Passed case count.
    pub passed: u64,
    /// Failed case count.
    pub failed: u64,
    /// Unsupported case count.
    pub unsupported: u64,
    /// Invalid case count.
    pub invalid: u64,
    /// Harness-error case count.
    pub harness_error: u64,
    /// Skipped case count.
    pub skipped: u64,
    /// Not-applicable case count (outside the denominator).
    pub not_applicable: u64,
    /// Sum of all applicable non-passing dispositions.
    pub not_passing: u64,
    /// Passed cases executed from an erratum-corrected artifact.
    pub passed_under_erratum: u64,
    /// Counts for mandatory cases.
    pub mandatory: Counts,
    /// Counts for optional cases.
    pub optional: Counts,
    #[serde(default)]
    /// Counts partitioned by specification fragment identifier.
    pub by_spec: BTreeMap<String, Counts>,
    #[serde(default)]
    /// Counts partitioned by data-model identifier.
    pub by_datamodel: BTreeMap<String, Counts>,
}

impl Summary {
    #[must_use]
    /// Recomputes all aggregate counts from canonical case results.
    pub fn from_cases(cases: &[CaseResult]) -> Self {
        let mut summary = Self {
            total_selected: cases.len() as u64,
            ..Self::default()
        };
        for case in cases {
            let counts = match case.conformance {
                ConformanceLevel::Mandatory => &mut summary.mandatory,
                ConformanceLevel::Optional => &mut summary.optional,
            };
            counts.add(case);
            summary
                .by_spec
                .entry(case.spec_id.clone())
                .or_default()
                .add(case);
            summary
                .by_datamodel
                .entry(case.datamodel.clone())
                .or_default()
                .add(case);
            match case.disposition {
                Disposition::Passed => {
                    summary.applicable_selected += 1;
                    summary.passed += 1;
                    if case.erratum.is_some() {
                        summary.passed_under_erratum += 1;
                    }
                }
                Disposition::Failed => {
                    summary.applicable_selected += 1;
                    summary.failed += 1;
                    summary.not_passing += 1;
                }
                Disposition::Unsupported => {
                    summary.applicable_selected += 1;
                    summary.unsupported += 1;
                    summary.not_passing += 1;
                }
                Disposition::Invalid => {
                    summary.applicable_selected += 1;
                    summary.invalid += 1;
                    summary.not_passing += 1;
                }
                Disposition::HarnessError => {
                    summary.applicable_selected += 1;
                    summary.harness_error += 1;
                    summary.not_passing += 1;
                }
                Disposition::Skipped => summary.skipped += 1,
                Disposition::NotApplicable => summary.not_applicable += 1,
            }
        }
        summary
    }

    #[must_use]
    /// Formats the stable human-readable count summary.
    pub fn concise(&self) -> String {
        format!(
            "Passed / Applicable Selected IRP = {}/{} ({} under documented errata); Not Passing = {} (failed {}, unsupported {}, invalid {}, harness-error {}); mandatory {}/{} passed with {} not passing; optional {}/{} passed with {} not passing; skipped {}; not applicable {}",
            self.passed,
            self.applicable_selected,
            self.passed_under_erratum,
            self.not_passing,
            self.failed,
            self.unsupported,
            self.invalid,
            self.harness_error,
            self.mandatory.passed,
            self.mandatory.applicable_selected,
            self.mandatory.not_passing,
            self.optional.passed,
            self.optional.applicable_selected,
            self.optional.not_passing,
            self.skipped,
            self.not_applicable
        )
    }

    #[must_use]
    /// Returns whether every applicable mandatory case passed.
    pub fn mandatory_complete(&self) -> bool {
        self.mandatory.not_passing == 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Canonical report for one selected conformance run.
pub struct RunReport {
    /// Report schema version.
    pub report_version: u32,
    /// Exact prepared-suite identity.
    pub suite: SuiteIdentity,
    /// Schema-validation policy used by the canonical compiler.
    pub schema_policy: scxml_reporting::ConformanceSchemaPolicy,
    /// Immutable canonical compiler implementation identity.
    pub compiler: scxml_reporting::ConformanceImplementationIdentity,
    /// Immutable deterministic engine implementation identity.
    pub engine: scxml_reporting::ConformanceImplementationIdentity,
    /// Immutable selected data-model implementation identity.
    pub data_model: scxml_reporting::ConformanceImplementationIdentity,
    /// Immutable conformance runner implementation identity.
    pub runner: scxml_reporting::ConformanceImplementationIdentity,
    /// Immutable manual-equivalent oracle table identity (name, table
    /// version, and source digest).
    pub manual_oracle: scxml_reporting::ConformanceImplementationIdentity,
    /// The extension packages the run bound (namespace URI, package
    /// version, and the SHA-256 of the manifest, schema, and component
    /// bytes), sorted by namespace URI; empty when it bound none.
    pub extensions: Vec<scxml_reporting::ConformanceExtensionIdentity>,
    /// Lowered machine IR schema version.
    pub ir_version: u32,
    /// Case selectors used for this run.
    pub filters: Filters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional source revision recorded by the caller.
    pub git_commit: Option<String>,
    /// SHA-256 binding of canonical report identity, filters, provenance, and results.
    pub identity_digest: String,
    /// Recomputed aggregate counts.
    pub summary: Summary,
    /// Deterministically ordered case results.
    pub cases: Vec<CaseResult>,
}

impl RunReport {
    #[allow(
        clippy::too_many_lines,
        reason = "one fail-closed pass validates every report field, count, and identity invariant"
    )]
    fn validate(&self) -> Result<(), ReportError> {
        let require_string = |field: &'static str, value: &str| -> Result<(), ReportError> {
            if value.is_empty() || value.len() > MAX_REPORT_STRING_BYTES {
                return Err(ReportError::new(
                    "report-string",
                    format!(
                        "{field} must be non-empty and at most {MAX_REPORT_STRING_BYTES} UTF-8 bytes"
                    ),
                ));
            }
            Ok(())
        };
        let require_collection = |field: &'static str, length: usize| -> Result<(), ReportError> {
            if length > MAX_REPORT_COLLECTION_ITEMS {
                return Err(ReportError::new(
                    "report-collection",
                    format!("{field} exceeds the maximum of {MAX_REPORT_COLLECTION_ITEMS} items"),
                ));
            }
            Ok(())
        };
        let require_digest = |field: &'static str, value: &str| -> Result<(), ReportError> {
            if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(ReportError::new(
                    "report-digest",
                    format!("{field} must be a 64-character hexadecimal SHA-256 digest"),
                ));
            }
            Ok(())
        };

        if self.report_version != scxml_reporting::CONFORMANCE_REPORT_VERSION {
            return Err(ReportError::new(
                "report-version",
                format!(
                    "unsupported report version {}; expected {}",
                    self.report_version,
                    scxml_reporting::CONFORMANCE_REPORT_VERSION
                ),
            ));
        }
        if self.ir_version == 0 {
            return Err(ReportError::new(
                "report-ir-version",
                "ir_version must be greater than zero",
            ));
        }
        require_string("suite.id", &self.suite.id)?;
        require_string("suite.version", &self.suite.version)?;
        require_string("suite.source", &self.suite.source)?;
        require_string("suite.datamodel", &self.suite.datamodel)?;
        for (field, implementation) in [
            ("compiler", &self.compiler),
            ("engine", &self.engine),
            ("data_model", &self.data_model),
            ("runner", &self.runner),
            ("manual_oracle", &self.manual_oracle),
        ] {
            require_string(field, &implementation.name)?;
            require_string(field, &implementation.version)?;
            require_digest(field, &implementation.source_digest)?;
        }
        require_collection("extensions", self.extensions.len())?;
        for extension in &self.extensions {
            require_string("extensions", &extension.namespace_uri)?;
            require_string("extensions", &extension.package_version)?;
            require_digest("extensions", &extension.manifest_sha256)?;
            require_digest("extensions", &extension.schema_sha256)?;
            require_digest("extensions", &extension.component_sha256)?;
        }
        if !self
            .extensions
            .windows(2)
            .all(|pair| pair[0].namespace_uri < pair[1].namespace_uri)
        {
            return Err(ReportError::new(
                "report-extensions",
                "extension identities must be sorted by name and distinct",
            ));
        }
        require_digest("suite.corpus_digest", &self.suite.corpus_digest)?;
        require_digest("suite.source_digest", &self.suite.source_digest)?;
        require_digest("suite.manifest_digest", &self.suite.manifest_digest)?;
        require_collection("suite.template_digests", self.suite.template_digests.len())?;
        for (name, digest) in &self.suite.template_digests {
            require_string("suite.template_digests key", name)?;
            require_digest("suite.template_digests value", digest)?;
        }
        if let Some(revision) = &self.suite.revision {
            require_string("suite.revision", revision)?;
        }
        if let Some(provenance) = &self.suite.provenance {
            require_string("suite.provenance", provenance)?;
        }
        if let Some(transformer) = &self.suite.transformer {
            require_string("suite.transformer.name", &transformer.name)?;
            require_string("suite.transformer.version", &transformer.version)?;
            require_collection(
                "suite.transformer.command_argv",
                transformer.command_argv.len(),
            )?;
            if transformer.command_argv.is_empty() {
                return Err(ReportError::new(
                    "report-suite-transformer-command",
                    "transformer command argument vector must not be empty",
                ));
            }
            for argument in &transformer.command_argv {
                require_string("suite.transformer.command_argv argument", argument)?;
            }
            if let Some(digest) = &transformer.executable_digest {
                require_digest("suite.transformer.executable_digest", digest)?;
            }
        }
        if let Some(errata) = &self.suite.errata {
            require_string("suite.errata.source", &errata.source)?;
            require_digest("suite.errata.digest", &errata.digest)?;
            require_collection("suite.errata.applied", errata.applied.len())?;
            require_collection("suite.errata.not_applicable", errata.not_applicable.len())?;
            for (case_id, erratum_id) in &errata.applied {
                require_string("suite.errata.applied case", case_id)?;
                require_string("suite.errata.applied erratum", erratum_id)?;
            }
            for case_id in &errata.not_applicable {
                require_string("suite.errata.not_applicable case", case_id)?;
            }
            if self.suite.kind != crate::manifest::CorpusKind::OfficialIrp {
                return Err(ReportError::new(
                    "report-suite-errata",
                    "only official IRP corpora may carry errata",
                ));
            }
        }
        if matches!(
            self.suite.kind,
            crate::manifest::CorpusKind::OfficialIrp | crate::manifest::CorpusKind::NamespaceIrp
        ) && (self.suite.transformer.is_none() || self.suite.template_digests.is_empty())
        {
            return Err(ReportError::new(
                "report-suite-identity",
                "official IRP reports require transformer and template identities",
            ));
        }
        if let Some(git_commit) = &self.git_commit {
            require_string("git_commit", git_commit)?;
        }
        require_digest("identity_digest", &self.identity_digest)?;

        require_collection("filters.ids", self.filters.ids.len())?;
        require_collection("filters.id_ranges", self.filters.id_ranges.len())?;
        require_collection("filters.spec_ids", self.filters.spec_ids.len())?;
        require_collection("filters.spec_nums", self.filters.spec_nums.len())?;
        require_collection("filters.conformance", self.filters.conformance.len())?;
        require_collection("filters.datamodels", self.filters.datamodels.len())?;
        require_collection("filters.tags", self.filters.tags.len())?;
        for value in self
            .filters
            .ids
            .iter()
            .chain(&self.filters.spec_ids)
            .chain(&self.filters.spec_nums)
            .chain(&self.filters.datamodels)
            .chain(&self.filters.tags)
        {
            require_string("filter value", value)?;
        }
        if self
            .filters
            .id_ranges
            .iter()
            .any(|range| range.start > range.end)
        {
            return Err(ReportError::new(
                "report-filter-range",
                "filter id ranges must have start less than or equal to end",
            ));
        }
        if self.cases.len() > scxml_reporting::MAX_CONFORMANCE_CASES {
            return Err(ReportError::new(
                "report-cases",
                format!(
                    "cases exceed the maximum of {} items",
                    scxml_reporting::MAX_CONFORMANCE_CASES
                ),
            ));
        }
        require_collection("summary.by_spec", self.summary.by_spec.len())?;
        require_collection("summary.by_datamodel", self.summary.by_datamodel.len())?;
        for key in self
            .summary
            .by_spec
            .keys()
            .chain(self.summary.by_datamodel.keys())
        {
            require_string("summary group key", key)?;
        }

        let mut case_ids = BTreeSet::new();
        for case in &self.cases {
            require_string("case.id", &case.id)?;
            if !case_ids.insert(case.id.as_str()) {
                return Err(ReportError::new(
                    "report-duplicate-case-id",
                    "case ids must be unique",
                ));
            }
            require_string("case.assertion_id", &case.assertion_id)?;
            require_string("case.spec_num", &case.spec_num)?;
            require_string("case.spec_id", &case.spec_id)?;
            require_string("case.datamodel", &case.datamodel)?;
            require_collection("case.diagnostics", case.diagnostics.len())?;
            require_collection("case.configuration", case.configuration.len())?;
            for diagnostic in &case.diagnostics {
                require_string("diagnostic.code", &diagnostic.code)?;
                require_string("diagnostic.severity", &diagnostic.severity)?;
                require_string("diagnostic.message", &diagnostic.message)?;
                if !matches!(diagnostic.severity.as_str(), "note" | "warning" | "error") {
                    return Err(ReportError::new(
                        "report-diagnostic-severity",
                        "diagnostic severity must be note, warning, or error",
                    ));
                }
            }
            let mut configuration = BTreeSet::new();
            for state in &case.configuration {
                require_string("case.configuration item", state)?;
                if !configuration.insert(state.as_str()) {
                    return Err(ReportError::new(
                        "report-duplicate-configuration",
                        "case configuration state ids must be unique",
                    ));
                }
            }
            if let Some(reason_code) = &case.reason_code {
                require_string("case.reason_code", reason_code)?;
                if !reason_code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                {
                    return Err(ReportError::new(
                        "report-reason-code",
                        "case reason_code must use lowercase ASCII letters, digits, and hyphens",
                    ));
                }
            }
            if let Some(reason) = &case.reason {
                require_string("case.reason", reason)?;
            }
            if case.terminal_status == Some(TerminalStatus::Rejected)
                && !(case.commands == 0 && case.fuel_consumed == 0 && case.configuration.is_empty())
            {
                return Err(ReportError::new(
                    "report-case-invariant",
                    "a rejected document cannot record configuration, commands, or fuel",
                ));
            }
            match case.disposition {
                // Whether a run must exist is the oracle's rule
                // (`validate_case_oracle`): the document-rejection oracle
                // decides without one.
                Disposition::Passed
                    if case.stage == Stage::Complete
                        && case.reason_code.is_none()
                        && case.reason.is_none() => {}
                Disposition::NotApplicable
                    if case.stage == Stage::Selection
                        && case.reason_code.as_deref() == Some(NOT_APPLICABLE_REASON_CODE)
                        && case.reason.is_some()
                        && case.commands == 0
                        && case.fuel_consumed == 0
                        && case.terminal_status.is_none()
                        && case.configuration.is_empty() => {}
                Disposition::Skipped
                    if case.stage == Stage::Selection
                        && case.manual
                        && case.reason_code.is_some()
                        && case.reason.is_some()
                        && case.commands == 0
                        && case.fuel_consumed == 0
                        && case.terminal_status.is_none()
                        && case.configuration.is_empty() => {}
                Disposition::Failed
                    if matches!(case.stage, Stage::Execution | Stage::Oracle)
                        && case.reason_code.is_some()
                        && case.reason.is_some() => {}
                Disposition::Unsupported
                    if matches!(case.stage, Stage::Capability | Stage::Oracle)
                        && case.reason_code.is_some()
                        && case.reason.is_some()
                        && case.commands == 0
                        && case.fuel_consumed == 0
                        && case.terminal_status.is_none()
                        && case.configuration.is_empty() => {}
                Disposition::Unsupported
                    if case.stage == Stage::Execution
                        && case.reason_code.is_some()
                        && case.reason.is_some()
                        && case.terminal_status.is_none()
                        && case.configuration.is_empty() => {}
                Disposition::Invalid
                    if matches!(
                        case.stage,
                        Stage::Integrity | Stage::Parse | Stage::Validation | Stage::Lowering
                    ) && case.reason_code.is_some()
                        && case.reason.is_some() => {}
                Disposition::HarnessError
                    if matches!(
                        case.stage,
                        Stage::Preparation | Stage::Integrity | Stage::Lowering | Stage::Execution
                    ) && case.reason_code.is_some()
                        && case.reason.is_some() => {}
                _ => {
                    return Err(ReportError::new(
                        "report-case-invariant",
                        "case disposition, stage, reason, and execution fields are inconsistent",
                    ));
                }
            }
            validate_case_oracle(self.suite.kind, case)?;
            validate_case_errata(&self.suite, case)?;
        }
        let numeric_case_id = |case: &CaseResult| {
            let digits = case.id.bytes().take_while(u8::is_ascii_digit).count();
            case.id
                .get(..digits)
                .and_then(|prefix| prefix.parse().ok())
                .unwrap_or(u64::MAX)
        };
        if !self.cases.windows(2).all(|pair| {
            (numeric_case_id(&pair[0]), pair[0].id.as_str())
                < (numeric_case_id(&pair[1]), pair[1].id.as_str())
        }) {
            return Err(ReportError::new(
                "report-case-order",
                "cases must use canonical numeric-then-lexical id order",
            ));
        }
        if Summary::from_cases(&self.cases) != self.summary {
            return Err(ReportError::new(
                "report-summary",
                "report summary does not equal the counts recomputed from cases",
            ));
        }
        if report_identity_digest(self)? != self.identity_digest {
            return Err(ReportError::new(
                "report-identity-tampered",
                "recomputed report identity does not match report metadata",
            ));
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the canonical envelope maps every report field in its fixed serialization order"
    )]
    /// Converts this report into the versioned canonical reporting envelope.
    ///
    /// # Errors
    ///
    /// Returns [`scxml_reporting::ArtifactError`] when report data violates the
    /// canonical reporting artifact's version, size, count, or field bounds.
    pub fn canonical_artifact(
        &self,
    ) -> Result<scxml_reporting::ArtifactEnvelope, scxml_reporting::ArtifactError> {
        self.canonical_conformance_payload()
            .and_then(scxml_reporting::ArtifactEnvelope::conformance)
            .and_then(|artifact| {
                let Some(payload) = artifact.conformance_payload() else {
                    return Err(scxml_reporting::ArtifactError::InvalidConformanceIdentity(
                        "conformance payload",
                    ));
                };
                if payload.identity.report_digest != self.identity_digest {
                    return Err(scxml_reporting::ArtifactError::ConformanceDigestMismatch);
                }
                Ok(artifact)
            })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the canonical payload maps every identity, summary, and case field in its fixed order"
    )]
    fn canonical_conformance_payload(
        &self,
    ) -> Result<scxml_reporting::ConformanceArtifact, scxml_reporting::ArtifactError> {
        let convert_counts = |counts: &Counts| scxml_reporting::ConformanceCounts {
            applicable_selected: counts.applicable_selected,
            passed: counts.passed,
            failed: counts.failed,
            unsupported: counts.unsupported,
            invalid: counts.invalid,
            harness_error: counts.harness_error,
            skipped: counts.skipped,
            not_applicable: counts.not_applicable,
            not_passing: counts.not_passing,
            passed_under_erratum: counts.passed_under_erratum,
        };
        let cases = self
            .cases
            .iter()
            .map(|case| {
                let diagnostics = case
                    .diagnostics
                    .iter()
                    .map(|diagnostic| {
                        let severity = match diagnostic.severity.as_str() {
                            "note" => scxml_reporting::Severity::Note,
                            "warning" => scxml_reporting::Severity::Warning,
                            "error" => scxml_reporting::Severity::Error,
                            _ => {
                                return Err(
                                    scxml_reporting::ArtifactError::InvalidConformanceCase(
                                        "case.diagnostic.severity",
                                    ),
                                );
                            }
                        };
                        Ok(scxml_reporting::ConformanceDiagnostic {
                            code: diagnostic.code.clone(),
                            severity,
                            message: diagnostic.message.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(scxml_reporting::ConformanceCase {
                    id: case.id.clone(),
                    assertion_id: case.assertion_id.clone(),
                    spec_num: case.spec_num.clone(),
                    spec_id: case.spec_id.clone(),
                    datamodel: case.datamodel.clone(),
                    conformance: match case.conformance {
                        ConformanceLevel::Mandatory => scxml_reporting::ConformanceLevel::Mandatory,
                        ConformanceLevel::Optional => scxml_reporting::ConformanceLevel::Optional,
                    },
                    manual: case.manual,
                    stage: match case.stage {
                        Stage::Selection => scxml_reporting::ConformanceStage::Selection,
                        Stage::Preparation => scxml_reporting::ConformanceStage::Preparation,
                        Stage::Integrity => scxml_reporting::ConformanceStage::Integrity,
                        Stage::Parse => scxml_reporting::ConformanceStage::Parse,
                        Stage::Validation => scxml_reporting::ConformanceStage::Validation,
                        Stage::Lowering => scxml_reporting::ConformanceStage::Lowering,
                        Stage::Capability => scxml_reporting::ConformanceStage::Capability,
                        Stage::Execution => scxml_reporting::ConformanceStage::Execution,
                        Stage::Oracle => scxml_reporting::ConformanceStage::Oracle,
                        Stage::Complete => scxml_reporting::ConformanceStage::Complete,
                    },
                    disposition: match case.disposition {
                        Disposition::Passed => scxml_reporting::ConformanceDisposition::Passed,
                        Disposition::Failed => scxml_reporting::ConformanceDisposition::Failed,
                        Disposition::Unsupported => {
                            scxml_reporting::ConformanceDisposition::Unsupported
                        }
                        Disposition::Invalid => scxml_reporting::ConformanceDisposition::Invalid,
                        Disposition::HarnessError => {
                            scxml_reporting::ConformanceDisposition::HarnessError
                        }
                        Disposition::Skipped => scxml_reporting::ConformanceDisposition::Skipped,
                        Disposition::NotApplicable => {
                            scxml_reporting::ConformanceDisposition::NotApplicable
                        }
                    },
                    reason_code: case.reason_code.clone(),
                    reason: case.reason.clone(),
                    diagnostics,
                    configuration: case.configuration.clone(),
                    terminal_status: case.terminal_status.map(|status| match status {
                        TerminalStatus::Running => {
                            scxml_reporting::ConformanceTerminalStatus::Running
                        }
                        TerminalStatus::Finished => {
                            scxml_reporting::ConformanceTerminalStatus::Finished
                        }
                        TerminalStatus::Rejected => {
                            scxml_reporting::ConformanceTerminalStatus::Rejected
                        }
                    }),
                    commands: case.commands,
                    fuel_consumed: case.fuel_consumed,
                    oracle: case.oracle.map(|oracle| match oracle {
                        OracleKind::OutcomeLog => {
                            scxml_reporting::ConformanceOracleKind::OutcomeLog
                        }
                        OracleKind::ManualEquivalent => {
                            scxml_reporting::ConformanceOracleKind::ManualEquivalent
                        }
                        OracleKind::SyntheticExpectation => {
                            scxml_reporting::ConformanceOracleKind::SyntheticExpectation
                        }
                        OracleKind::DocumentRejection => {
                            scxml_reporting::ConformanceOracleKind::DocumentRejection
                        }
                    }),
                    erratum: case.erratum.clone(),
                })
            })
            .collect::<Result<Vec<_>, scxml_reporting::ArtifactError>>()?;
        let summary = scxml_reporting::ConformanceSummary {
            total_selected: self.summary.total_selected,
            counts: scxml_reporting::ConformanceCounts {
                applicable_selected: self.summary.applicable_selected,
                passed: self.summary.passed,
                failed: self.summary.failed,
                unsupported: self.summary.unsupported,
                invalid: self.summary.invalid,
                harness_error: self.summary.harness_error,
                skipped: self.summary.skipped,
                not_applicable: self.summary.not_applicable,
                not_passing: self.summary.not_passing,
                passed_under_erratum: self.summary.passed_under_erratum,
            },
            mandatory: convert_counts(&self.summary.mandatory),
            optional: convert_counts(&self.summary.optional),
            by_spec: self
                .summary
                .by_spec
                .iter()
                .map(|(key, counts)| (key.clone(), convert_counts(counts)))
                .collect(),
            by_datamodel: self
                .summary
                .by_datamodel
                .iter()
                .map(|(key, counts)| (key.clone(), convert_counts(counts)))
                .collect(),
        };
        Ok(scxml_reporting::ConformanceArtifact {
            report_version: self.report_version,
            identity: scxml_reporting::ConformanceRunIdentity {
                suite: scxml_reporting::ConformanceSuiteIdentity {
                    id: self.suite.id.clone(),
                    version: self.suite.version.clone(),
                    kind: match self.suite.kind {
                        crate::manifest::CorpusKind::OfficialIrp => {
                            scxml_reporting::ConformanceCorpusKind::OfficialIrp
                        }
                        crate::manifest::CorpusKind::SyntheticHarness => {
                            scxml_reporting::ConformanceCorpusKind::SyntheticHarness
                        }
                        crate::manifest::CorpusKind::NamespaceIrp => {
                            scxml_reporting::ConformanceCorpusKind::NamespaceIrp
                        }
                    },
                    manifest_source: self.suite.source.clone(),
                    revision: self.suite.revision.clone(),
                    corpus_digest: self.suite.corpus_digest.clone(),
                    source_set_digest: self.suite.source_digest.clone(),
                    manifest_digest: self.suite.manifest_digest.clone(),
                    template_digests: self.suite.template_digests.clone(),
                    transformer: self.suite.transformer.as_ref().map(|transformer| {
                        scxml_reporting::ConformanceTransformerIdentity {
                            name: transformer.name.clone(),
                            version: transformer.version.clone(),
                            command_argv: transformer.command_argv.clone(),
                            executable_digest: transformer.executable_digest.clone(),
                        }
                    }),
                    datamodel: self.suite.datamodel.clone(),
                    provenance: self.suite.provenance.clone(),
                    errata: self.suite.errata.as_ref().map(|errata| {
                        scxml_reporting::ConformanceErrataIdentity {
                            source: errata.source.clone(),
                            digest: errata.digest.clone(),
                            applied: errata.applied.clone(),
                            not_applicable: errata.not_applicable.clone(),
                        }
                    }),
                },
                schema_policy: self.schema_policy,
                compiler: self.compiler.clone(),
                engine: self.engine.clone(),
                data_model: self.data_model.clone(),
                runner: self.runner.clone(),
                manual_oracle: self.manual_oracle.clone(),
                extensions: self.extensions.clone(),
                ir_version: self.ir_version,
                filters: scxml_reporting::ConformanceFilters {
                    ids: self.filters.ids.clone(),
                    id_ranges: self
                        .filters
                        .id_ranges
                        .iter()
                        .map(|range| scxml_reporting::ConformanceIdRange {
                            start: range.start,
                            end: range.end,
                        })
                        .collect(),
                    spec_ids: self.filters.spec_ids.clone(),
                    spec_nums: self.filters.spec_nums.clone(),
                    conformance: self
                        .filters
                        .conformance
                        .iter()
                        .map(|level| match level {
                            ConformanceLevel::Mandatory => {
                                scxml_reporting::ConformanceLevel::Mandatory
                            }
                            ConformanceLevel::Optional => {
                                scxml_reporting::ConformanceLevel::Optional
                            }
                        })
                        .collect(),
                    datamodels: self.filters.datamodels.clone(),
                    tags: self.filters.tags.clone(),
                },
                run_revision: self.git_commit.clone(),
                report_digest: String::new(),
            },
            summary,
            cases,
        })
    }

    /// Validates and writes the report as canonical pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns [`ReportError`] when report invariants fail, serialization
    /// exceeds canonical bounds, or `path` cannot be written.
    pub fn write_json(&self, path: &Path) -> Result<(), ReportError> {
        let span = observe(Module::ConformanceReport, Operation::ReportWriteJson);
        let result = (|| {
            self.validate()?;
            let mut bytes = serde_json::to_vec_pretty(self)
                .map_err(|error| ReportError::new("report-json", error.to_string()))?;
            bytes.push(b'\n');
            if bytes.len() > scxml_reporting::MAX_CANONICAL_BYTES {
                return Err(ReportError::new(
                    "report-size",
                    format!(
                        "report exceeds the maximum of {} bytes",
                        scxml_reporting::MAX_CANONICAL_BYTES
                    ),
                ));
            }
            fs::write(path, bytes).map_err(|error| {
                ReportError::new(
                    "report-write",
                    format!("cannot write `{}`: {error}", path.display()),
                )
            })
        })();
        if result.is_err() {
            span.fail(ErrorCategory::Report);
        }
        result
    }

    /// Validates and returns the canonical pretty-JSON bytes the report
    /// would persist, without touching the filesystem; byte equality of
    /// these bytes is byte equality of persisted reports.
    ///
    /// # Errors
    ///
    /// As [`RunReport::write_json`], minus the filesystem write.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReportError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| ReportError::new("report-json", error.to_string()))?;
        bytes.push(b'\n');
        if bytes.len() > scxml_reporting::MAX_CANONICAL_BYTES {
            return Err(ReportError::new(
                "report-size",
                format!(
                    "report exceeds the maximum of {} bytes",
                    scxml_reporting::MAX_CANONICAL_BYTES
                ),
            ));
        }
        Ok(bytes)
    }

    /// Reads, validates, and canonical-encoding-checks a report JSON file.
    ///
    /// # Errors
    ///
    /// Returns [`ReportError`] when `path` is unavailable, not a bounded regular
    /// file, invalid JSON, semantically inconsistent, or not byte-canonical.
    pub fn read_json(path: &Path) -> Result<Self, ReportError> {
        let span = observe(Module::ConformanceReport, Operation::ReportReadJson);
        let result = (|| {
            let file = File::open(path).map_err(|error| {
                ReportError::new(
                    "report-read",
                    format!("cannot open `{}`: {error}", path.display()),
                )
            })?;
            let metadata = file.metadata().map_err(|error| {
                ReportError::new(
                    "report-read",
                    format!("cannot inspect `{}`: {error}", path.display()),
                )
            })?;
            let max_bytes = u64::try_from(scxml_reporting::MAX_CANONICAL_BYTES)
                .map_err(|_| ReportError::new("report-size", "report byte limit is unsupported"))?;
            if !metadata.is_file() || metadata.len() > max_bytes {
                return Err(ReportError::new(
                    "report-size",
                    format!(
                        "report must be a regular file no larger than {} bytes",
                        scxml_reporting::MAX_CANONICAL_BYTES
                    ),
                ));
            }
            let limit = max_bytes.saturating_add(1);
            let mut bytes = Vec::new();
            file.take(limit).read_to_end(&mut bytes).map_err(|error| {
                ReportError::new(
                    "report-read",
                    format!("cannot read `{}`: {error}", path.display()),
                )
            })?;
            if bytes.len() > scxml_reporting::MAX_CANONICAL_BYTES {
                return Err(ReportError::new(
                    "report-size",
                    format!(
                        "report exceeds the maximum of {} bytes",
                        scxml_reporting::MAX_CANONICAL_BYTES
                    ),
                ));
            }
            let report: Self = serde_json::from_slice(&bytes).map_err(|error| {
                ReportError::new(
                    "report-json",
                    format!("cannot parse `{}`: {error}", path.display()),
                )
            })?;
            report.validate()?;
            let mut canonical = serde_json::to_vec_pretty(&report)
                .map_err(|error| ReportError::new("report-json", error.to_string()))?;
            canonical.push(b'\n');
            if canonical != bytes {
                return Err(ReportError::new(
                    "report-noncanonical",
                    "report bytes do not match the canonical pretty JSON encoding",
                ));
            }
            let report_bytes = u64::try_from(bytes.len())
                .map_err(|_| ReportError::new("report-size", "report byte count is unsupported"))?;
            let case_count = u64::try_from(report.cases.len()).map_err(|_| {
                ReportError::new("report-cases", "report case count is unsupported")
            })?;
            record_artifact_measurement(ArtifactMeasurement::ReportBytes, report_bytes);
            record_artifact_measurement(ArtifactMeasurement::ReportCases, case_count);
            span.event(scxml_telemetry::SignalEvent::ArtifactAccepted);
            Ok(report)
        })();
        if result.is_err() {
            span.fail(ErrorCategory::Report);
        }
        result
    }
}

/// Checks that a case's oracle is present exactly when execution reached
/// oracle evaluation and that its kind matches the corpus provenance and the
/// documented manual-equivalent table.
fn validate_case_oracle(
    corpus_kind: crate::manifest::CorpusKind,
    case: &CaseResult,
) -> Result<(), ReportError> {
    let evaluated = matches!(
        (case.disposition, case.stage),
        (Disposition::Passed, Stage::Complete) | (Disposition::Failed, Stage::Oracle)
    );
    let documented_manual = case.manual
        && case.assertion_id == case.id
        && (crate::manual_oracle::ManualOracle::ALL
            .iter()
            .any(|oracle| oracle.case_id() == case.id)
            || crate::external_client::ORACLES
                .iter()
                .any(|oracle| oracle.case_id == case.id));
    let ran = case.terminal_status.is_some();
    let never_ran = case.terminal_status.is_none()
        && case.commands == 0
        && case.fuel_consumed == 0
        && case.configuration.is_empty();
    let consistent = match (corpus_kind, case.oracle) {
        (_, None) => !evaluated,
        (crate::manifest::CorpusKind::OfficialIrp, Some(OracleKind::OutcomeLog)) => {
            evaluated && ran && !documented_manual
        }
        (crate::manifest::CorpusKind::OfficialIrp, Some(OracleKind::ManualEquivalent)) => {
            evaluated && ran && documented_manual
        }
        (crate::manifest::CorpusKind::NamespaceIrp, Some(OracleKind::OutcomeLog))
        | (crate::manifest::CorpusKind::SyntheticHarness, Some(OracleKind::SyntheticExpectation)) => {
            evaluated && ran
        }
        (crate::manifest::CorpusKind::NamespaceIrp, Some(OracleKind::DocumentRejection)) => {
            evaluated && never_ran
        }
        _ => false,
    };
    if consistent {
        Ok(())
    } else {
        Err(ReportError::new(
            "report-case-oracle",
            "case oracle kind is inconsistent with its disposition, stage, provenance, or the manual-equivalent table",
        ))
    }
}

/// Checks that a case's erratum and not-applicable disposition agree exactly
/// with the suite's recorded errata identity.
fn validate_case_errata(
    suite: &crate::manifest::SuiteIdentity,
    case: &CaseResult,
) -> Result<(), ReportError> {
    let errata = suite.errata.as_ref();
    let recorded = errata.and_then(|errata| errata.applied.get(&case.id));
    let declared = errata.is_some_and(|errata| errata.not_applicable.contains(&case.id));
    let consistent = case.erratum.as_ref() == recorded
        && declared == (case.disposition == Disposition::NotApplicable)
        && !(declared && case.erratum.is_some());
    if consistent {
        Ok(())
    } else {
        Err(ReportError::new(
            "report-case-errata",
            "case erratum or not-applicable disposition is inconsistent with the suite errata identity",
        ))
    }
}

/// Computes the SHA-256 binding for a canonical run report.
///
/// The shared reporting contract covers the report version, suite metadata,
/// runner/manual-oracle/IR versions, filters, caller provenance, summary, and every ordered
/// producer case observation. Only the digest field itself is cleared before
/// hashing.
///
/// # Errors
///
/// Returns [`ReportError`] when canonical serialization fails.
pub fn report_identity_digest(report: &RunReport) -> Result<String, ReportError> {
    let mut canonical = report.clone();
    canonical.identity_digest = "0".repeat(64);
    let payload = canonical
        .canonical_conformance_payload()
        .map_err(|error| ReportError::new("report-identity-encode", error.to_string()))?;
    scxml_reporting::conformance_report_digest(&payload)
        .map_err(|error| ReportError::new("report-identity-encode", error.to_string()))
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Case-ID changes between compatible baseline and current reports.
pub struct Comparison {
    /// Previously non-passing cases that now pass.
    pub new_passes: Vec<String>,
    /// Previously passing cases that are now non-passing.
    pub regressions: Vec<String>,
    /// Previously unsupported cases that now execute but fail.
    pub newly_supported_failing: Vec<String>,
    /// Previously supported cases that are now unsupported.
    pub newly_unsupported: Vec<String>,
    /// Previously invalid cases that are no longer invalid.
    pub fixed_invalid: Vec<String>,
    /// Case identifiers present only in the current report.
    pub added: Vec<String>,
    /// Case identifiers present only in the baseline report.
    pub removed: Vec<String>,
}

impl Comparison {
    #[must_use]
    /// Returns whether any prior pass became non-passing.
    pub fn has_regressions(&self) -> bool {
        !self.regressions.is_empty()
    }

    #[must_use]
    /// Formats stable counts for every comparison category.
    pub fn concise(&self) -> String {
        format!(
            "baseline: new-passes {}; regressions {}; newly-supported-failing {}; newly-unsupported {}; fixed-invalid {}; added {}; removed {}",
            self.new_passes.len(),
            self.regressions.len(),
            self.newly_supported_failing.len(),
            self.newly_unsupported.len(),
            self.fixed_invalid.len(),
            self.added.len(),
            self.removed.len()
        )
    }
}

/// Whether a baseline comparison requires matching run identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityPolicy {
    /// Reject baselines whose corpus, transformer, schema, compiler, engine,
    /// data-model, runner, IR, or filter identity differs (the default).
    Require,
    /// Explicitly opt into an exploratory comparison across identities.
    AllowMismatch,
}

/// Compares case dispositions between two reports.
///
/// Identity equality is required unless `identity_policy` is
/// [`IdentityPolicy::AllowMismatch`].
///
/// # Errors
///
/// Returns [`ReportError`] when identities differ without the override, either
/// report is invalid, or duplicate case identifiers prevent an unambiguous
/// comparison.
pub fn compare_reports(
    baseline: &RunReport,
    current: &RunReport,
    identity_policy: IdentityPolicy,
) -> Result<Comparison, ReportError> {
    if identity_policy == IdentityPolicy::Require
        && (baseline.suite != current.suite
            || baseline.schema_policy != current.schema_policy
            || baseline.compiler != current.compiler
            || baseline.engine != current.engine
            || baseline.data_model != current.data_model
            || baseline.runner != current.runner
            || baseline.ir_version != current.ir_version
            || baseline.filters != current.filters)
    {
        return Err(ReportError::new(
            "baseline-identity-mismatch",
            "baseline corpus/transformer/schema/compiler/engine/data-model/runner/IR/filter identity differs; pass explicit override only for intentional exploratory comparison",
        ));
    }
    let old: BTreeMap<_, _> = baseline
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case.disposition))
        .collect();
    let new: BTreeMap<_, _> = current
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case.disposition))
        .collect();
    let old_ids: BTreeSet<_> = old.keys().copied().collect();
    let new_ids: BTreeSet<_> = new.keys().copied().collect();
    let mut comparison = Comparison {
        added: new_ids
            .difference(&old_ids)
            .map(|id| (*id).to_owned())
            .collect(),
        removed: old_ids
            .difference(&new_ids)
            .map(|id| (*id).to_owned())
            .collect(),
        ..Comparison::default()
    };
    for id in old_ids.intersection(&new_ids) {
        let before = old[id];
        let after = new[id];
        if before != Disposition::Passed && after == Disposition::Passed {
            comparison.new_passes.push((*id).to_owned());
        }
        if before == Disposition::Passed && after != Disposition::Passed {
            comparison.regressions.push((*id).to_owned());
        }
        if matches!(before, Disposition::Unsupported | Disposition::Invalid)
            && after == Disposition::Failed
        {
            comparison.newly_supported_failing.push((*id).to_owned());
        }
        if before != Disposition::Unsupported && after == Disposition::Unsupported {
            comparison.newly_unsupported.push((*id).to_owned());
        }
        if before == Disposition::Invalid && after != Disposition::Invalid {
            comparison.fixed_invalid.push((*id).to_owned());
        }
    }
    Ok(comparison)
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// A bounded report validation, serialization, comparison, or I/O failure.
pub struct ReportError {
    /// Stable machine-readable failure code.
    pub code: &'static str,
    /// Human-readable failure description.
    pub message: String,
}

impl ReportError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ReportError {}
