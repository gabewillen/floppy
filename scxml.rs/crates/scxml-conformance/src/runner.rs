use std::fmt;
use std::path::Path;

use scxml_compiler_core::{CompileInput, Compiler, MACHINE_IR_VERSION, MachineIr};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_validator_core::{FileSchemaResolver, SchemaMode, Severity, Validator};
use scxml_validator_xsd::XsdSchemaBackend;

use crate::capability;
use crate::executor::{
    CaseExecutor, CaseSnapshotDocument, ExecutionFailureCategory, implementation_identity,
};
use crate::external_client::{self, ExternalClientOracle};
use crate::filter::Filters;
use crate::manifest::{
    CorpusKind, ExpectedOutcome, LoadedCase, LoadedSuite, ManifestError, load_suite,
};
use crate::manual_oracle::{MANUAL_ORACLE_NAME, MANUAL_ORACLE_VERSION};
use crate::oracle;
use crate::report::{
    CaseResult, DiagnosticRecord, Disposition, NOT_APPLICABLE_REASON_CODE, OracleKind,
    RUNNER_VERSION, ReportError, RunReport, Stage, Summary, report_identity_digest,
};

const REPORT_VERSION: u32 = scxml_reporting::CONFORMANCE_REPORT_VERSION;

#[derive(Clone, Debug, Eq, PartialEq)]
/// Configuration for selecting and executing a prepared suite.
pub struct RunOptions {
    /// Case selectors applied before execution.
    pub filters: Filters,
    /// Schema-validation policy used by the canonical compiler.
    pub schema_mode: SchemaMode,
    /// Optional source revision recorded as report provenance.
    pub git_commit: Option<String>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            filters: Filters::default(),
            schema_mode: SchemaMode::Off,
            git_commit: None,
        }
    }
}

#[derive(Debug)]
/// Failure to load or initialize a conformance run.
pub enum RunnerError {
    /// Prepared corpus manifest loading or integrity validation failed.
    Manifest(ManifestError),
    /// Canonical report identity construction failed.
    Report(ReportError),
}

impl fmt::Display for RunnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(error) => error.fmt(formatter),
            Self::Report(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RunnerError {}

impl From<ManifestError> for RunnerError {
    fn from(value: ManifestError) -> Self {
        Self::Manifest(value)
    }
}

impl From<ReportError> for RunnerError {
    fn from(value: ReportError) -> Self {
        Self::Report(value)
    }
}

/// Loads a prepared corpus directory or manifest path and executes it
/// through `executor` (see [`run_suite`]).
///
/// # Errors
///
/// Returns [`RunnerError::Manifest`] when the suite manifest or any referenced
/// prepared artifact cannot be read, validated, or integrity-checked, and
/// [`RunnerError::Report`] as [`run_suite`].
pub fn run_corpus(
    path: &Path,
    options: &RunOptions,
    executor: &dyn CaseExecutor,
) -> Result<RunReport, RunnerError> {
    let span = observe(Module::ConformanceRunner, Operation::RunnerRunCorpus);
    let result = load_suite(path)
        .map_err(RunnerError::from)
        .and_then(|suite| run_suite(&suite, options, executor));
    if result.is_err() {
        span.fail(ErrorCategory::InputRead);
    }
    result
}

/// Executes already-loaded selected cases through `executor` and returns a
/// complete report.
///
/// Selection, compilation, capability detection, and every oracle are
/// runner-owned; only case execution and the engine, runner, and data-model
/// identities come from `executor`. Per-case parse, validation, lowering,
/// capability, execution, and oracle failures are represented as case
/// dispositions rather than function errors.
///
/// # Errors
///
/// Returns [`RunnerError::Report`] when the report identity cannot be encoded.
#[allow(
    clippy::too_many_lines,
    reason = "one pass dispatches every selected case to its source and binds every implementation identity into the report"
)]
pub fn run_suite(
    suite: &LoadedSuite,
    options: &RunOptions,
    executor: &dyn CaseExecutor,
) -> Result<RunReport, RunnerError> {
    run_suite_with_snapshot_sink(suite, options, executor, None)
}

/// Per-case snapshot sink of [`run_suite_with_snapshot_sink`]: called once
/// per executed case, in run order, with the case id and the documents the
/// executor collected at that case's checkpoints (empty unless the executor
/// collects snapshots; docs/session-snapshot.md, "Non-perturbation proof
/// over the IRP"). The sink observes only; the report
/// and every oracle are unchanged.
pub type SnapshotSink<'a> = &'a dyn Fn(&str, &[CaseSnapshotDocument]);

/// [`run_suite`], handing every executed case's collected snapshot
/// documents to `sink`.
///
/// # Errors
///
/// As [`run_suite`].
#[allow(
    clippy::too_many_lines,
    reason = "one pass dispatches every selected case to its source and binds every implementation identity into the report"
)]
pub fn run_suite_with_snapshot_sink(
    suite: &LoadedSuite,
    options: &RunOptions,
    executor: &dyn CaseExecutor,
    sink: Option<SnapshotSink<'_>>,
) -> Result<RunReport, RunnerError> {
    let span = observe(Module::ConformanceRunner, Operation::RunnerRunSuite);
    let extensions = executor.extensions();
    let validator = Validator::new(FileSchemaResolver::new(), XsdSchemaBackend)
        .with_schema_mode(options.schema_mode);
    // The `env` slots of every bound namespace's package (`SCXML083`); a run
    // that binds none judges no slot, as before extensions existed.
    let validator = if extensions.is_empty() {
        validator
    } else {
        validator.with_extension_slots(
            extensions
                .iter()
                .map(|bound| {
                    (
                        bound.manifest.namespace_uri.clone(),
                        bound.manifest.env_slots.clone(),
                    )
                })
                .collect(),
        )
    };
    let compiler = Compiler::new(validator);
    let mut cases = Vec::new();
    for case in &suite.cases {
        if !options.filters.selects(&case.manifest) {
            continue;
        }
        // Manual cases execute through the same pipeline as every other case;
        // the oracle decides their disposition, never the manual marker.
        // A W3C not-applicable declaration (recorded in the prepared errata
        // identity) removes the case from the applicable denominator; it is
        // never executed and never counts as passing.
        let result = if let Some(declaration) = &case.manifest.not_applicable {
            result(
                case,
                Stage::Selection,
                Disposition::NotApplicable,
                Some(NOT_APPLICABLE_REASON_CODE),
                Some(declaration.reason.clone()),
                Vec::new(),
                Vec::new(),
                None,
                0,
                0,
            )
        } else if let Some(failure) = &case.manifest.preparation_failure {
            result(
                case,
                Stage::Preparation,
                Disposition::HarnessError,
                Some(&failure.code),
                Some(failure.message.clone()),
                Vec::new(),
                Vec::new(),
                None,
                0,
                0,
            )
        } else if let Some(selected) = (suite.manifest.suite.kind == CorpusKind::OfficialIrp)
            .then(|| external_client::select(case))
            .flatten()
        {
            run_external_client_case(case, selected, suite, &compiler, executor, sink)
        } else if case.source.is_none() || case.source_path.is_none() {
            result(
                case,
                Stage::Preparation,
                Disposition::HarnessError,
                Some("prepared-source-missing"),
                Some("selected applicable case has no prepared SCXML artifact".into()),
                Vec::new(),
                Vec::new(),
                None,
                0,
                0,
            )
        } else {
            run_case(
                case,
                CaseSource::Prepared,
                &Suite {
                    kind: suite.manifest.suite.kind,
                    default_steps: suite.manifest.default_command_budget,
                    extensions: &extensions,
                },
                &compiler,
                executor,
                sink,
            )
        };
        cases.push(result);
    }
    let summary = Summary::from_cases(&cases);
    let identities = executor.identities();
    let data_model = identities.data_model(&suite.manifest.suite.datamodel);
    let mut report = RunReport {
        report_version: REPORT_VERSION,
        suite: suite.manifest.suite.clone(),
        schema_policy: schema_policy(options.schema_mode),
        compiler: implementation_identity(
            "scxml-compiler-core",
            RUNNER_VERSION,
            &[
                include_bytes!("../../scxml-compiler-core/src/lib.rs"),
                include_bytes!("../../scxml-compiler-core/src/ir.rs"),
                include_bytes!("../../scxml-compiler-core/src/lower.rs"),
            ],
        ),
        engine: identities.engine,
        data_model,
        runner: identities.runner,
        // The manual-equivalent oracle table carries its own version so a
        // changed checked condition is visible in every report identity.
        manual_oracle: implementation_identity(
            MANUAL_ORACLE_NAME,
            MANUAL_ORACLE_VERSION,
            &[
                include_bytes!("manual_oracle.rs"),
                include_bytes!("external_client.rs"),
            ],
        ),
        extensions: extensions
            .iter()
            .map(|bound| bound.identity.clone())
            .collect(),
        ir_version: MACHINE_IR_VERSION,
        filters: options.filters.clone(),
        git_commit: options.git_commit.clone(),
        identity_digest: String::new(),
        summary,
        cases,
    };
    let result = report_identity_digest(&report).map_err(RunnerError::from);
    match result {
        Ok(digest) => {
            report.identity_digest = digest;
            Ok(report)
        }
        Err(error) => {
            span.fail(ErrorCategory::Report);
            Err(error)
        }
    }
}

fn schema_policy(mode: SchemaMode) -> scxml_reporting::ConformanceSchemaPolicy {
    match mode {
        SchemaMode::Off => scxml_reporting::ConformanceSchemaPolicy::Off,
        SchemaMode::Lax => scxml_reporting::ConformanceSchemaPolicy::Lax,
        SchemaMode::Strict => scxml_reporting::ConformanceSchemaPolicy::Strict,
    }
}

#[allow(
    clippy::too_many_lines,
    clippy::similar_names,
    reason = "one staged case pipeline maps every stage failure to its disposition; stage bindings share prefixes by design"
)]
fn run_case<R, B>(
    case: &LoadedCase,
    source: CaseSource,
    suite: &Suite<'_>,
    compiler: &Compiler<R, B>,
    executor: &dyn CaseExecutor,
    sink: Option<SnapshotSink<'_>>,
) -> CaseResult
where
    R: scxml_validator_core::SchemaResolver,
    B: scxml_validator_core::SchemaValidator,
{
    let span = observe(Module::ConformanceRunner, Operation::RunnerRunCase);
    let (source_name, source_text) = match source {
        CaseSource::Prepared => (
            case.source_path
                .as_ref()
                .map(|path| path.display().to_string()),
            case.source.clone(),
        ),
        // The receiving session of an external-client case is the
        // harness-authored document named by its documented oracle.
        CaseSource::ExternalClient(client) => (
            Some(format!(
                "harness/external-client/{}-receiver.scxml",
                client.case_id
            )),
            Some(client.receiver(&case.manifest.datamodel)),
        ),
    };
    let Some(source_path) = source_name else {
        span.fail(ErrorCategory::Harness);
        return result(
            case,
            Stage::Preparation,
            Disposition::HarnessError,
            Some("missing-source-path"),
            Some("loaded case has no prepared source path".into()),
            Vec::new(),
            Vec::new(),
            None,
            0,
            0,
        );
    };
    let Some(source_text) = source_text else {
        span.fail(ErrorCategory::Harness);
        return result(
            case,
            Stage::Preparation,
            Disposition::HarnessError,
            Some("missing-source-text"),
            Some("loaded case has no prepared source text".into()),
            Vec::new(),
            Vec::new(),
            None,
            0,
            0,
        );
    };
    let corpus_kind = suite.kind;
    let compiled = compiler.compile(CompileInput {
        source_name: source_path,
        source: source_text,
        // Exactly the packages of the namespaces the executor binds.
        packages: suite
            .extensions
            .iter()
            .map(|bound| bound.manifest.clone())
            .collect(),
        package_schemas: suite
            .extensions
            .iter()
            .map(|bound| scxml_compiler_core::NamespaceSchemaBinding {
                namespace_uri: bound.manifest.namespace_uri.clone(),
                schema: bound.schema.clone(),
            })
            .collect(),
        // The official corpus supplies no script packages: `<script src>`
        // resolves at document load through the execution host's script
        // loader (runtime resolution keeps lowering free of package
        // diagnostics).
        scripts: Vec::new(),
        script_resolution: scxml_compiler_core::ScriptResolution::Runtime,
        // A namespace case's `event:schema` files are its declared,
        // integrity-checked dependencies at or below its start document's
        // directory, keyed relative to that directory, as the document
        // names them (the CLI reads them relative to the document the same
        // way); the W3C corpus has none.
        event_schemas: if corpus_kind == CorpusKind::NamespaceIrp {
            namespace_event_schemas(case)
        } else {
            std::collections::BTreeMap::new()
        },
    });
    let diagnostics = compiled
        .validation
        .report
        .diagnostics()
        .iter()
        .map(|diagnostic| DiagnosticRecord {
            code: diagnostic.code.clone(),
            severity: diagnostic.severity.as_str().into(),
            message: diagnostic.message.clone(),
        })
        .collect::<Vec<_>>();
    // A document the namespace requires to be rejected never runs: the
    // document-rejection oracle decides it from the compiler's verdict.
    if let ExpectedOutcome::DocumentRejection { codes } = &case.manifest.expected {
        let rejected = compiled.validation.document.is_none()
            || has_validation_errors(&compiled.validation.report);
        let error_codes = compiled
            .validation
            .report
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .map(|diagnostic| diagnostic.code.as_str());
        let (stage, disposition, failure) =
            match oracle::evaluate_document_rejection(codes, rejected, error_codes) {
                Ok(()) => (Stage::Complete, Disposition::Passed, None),
                Err(failure) => {
                    span.fail(ErrorCategory::Oracle);
                    (Stage::Oracle, Disposition::Failed, Some(failure))
                }
            };
        return CaseResult {
            oracle: Some(OracleKind::DocumentRejection),
            ..result(
                case,
                stage,
                disposition,
                failure.as_ref().map(|failure| failure.code),
                failure.map(|failure| failure.message),
                diagnostics,
                Vec::new(),
                None,
                0,
                0,
            )
        };
    }
    let Some(document) = compiled.validation.document.as_ref() else {
        span.fail(ErrorCategory::Parse);
        return result(
            case,
            Stage::Parse,
            Disposition::Invalid,
            Some("source-invalid"),
            Some("parser diagnostics rejected the case".into()),
            diagnostics,
            Vec::new(),
            None,
            0,
            0,
        );
    };
    if has_validation_errors(&compiled.validation.report) {
        span.fail(ErrorCategory::Validation);
        return result(
            case,
            Stage::Validation,
            Disposition::Invalid,
            Some("source-invalid"),
            Some("validator diagnostics rejected the case".into()),
            diagnostics,
            Vec::new(),
            None,
            0,
            0,
        );
    }
    span.event(SignalEvent::InputAccepted);
    let bound_namespaces = suite
        .extensions
        .iter()
        .map(|bound| bound.manifest.namespace_uri.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(unsupported) = capability::detect(case, document, &bound_namespaces) {
        span.fail(ErrorCategory::Unsupported);
        return result(
            case,
            unsupported.stage,
            Disposition::Unsupported,
            Some(unsupported.code),
            Some(unsupported.message),
            diagnostics,
            Vec::new(),
            None,
            0,
            0,
        );
    }
    let Some(artifact) = compiled.artifact else {
        span.fail(ErrorCategory::Lowering);
        return result(
            case,
            Stage::Lowering,
            Disposition::Invalid,
            Some("lowering-rejected"),
            Some("compiler did not produce Machine IR".into()),
            diagnostics,
            Vec::new(),
            None,
            0,
            0,
        );
    };
    let ir = match MachineIr::from_json(&artifact.bytes) {
        Ok(ir) => ir,
        Err(error) => {
            span.fail(ErrorCategory::Harness);
            return result(
                case,
                Stage::Lowering,
                Disposition::HarnessError,
                Some("ir-decode"),
                Some(error.to_string()),
                diagnostics,
                Vec::new(),
                None,
                0,
                0,
            );
        }
    };
    // External data sources come from the case's integrity-checked
    // dependency set; only UTF-8 `file:` payloads are usable as values.
    let mut data_sources = std::collections::BTreeMap::new();
    for (uri, bytes) in &case.dependency_sources {
        if let Ok(text) = String::from_utf8(bytes.clone()) {
            data_sources.insert(uri.clone(), text);
        }
    }
    let command_budget = case.manifest.max_commands.unwrap_or(suite.default_steps);
    let executed = match source {
        CaseSource::Prepared => executor
            .execute(
                &artifact.bytes,
                &ir,
                &case.manifest.events,
                command_budget,
                &data_sources,
            )
            .map(|executed| (executed, None)),
        // The harness acts as the W3C tester's external HTTP client.
        CaseSource::ExternalClient(client) => executor
            .execute_external_client(
                &artifact.bytes,
                &ir,
                &|location| client.request(location),
                command_budget,
                &data_sources,
            )
            .map(|run| (run.execution, Some((client, run.response)))),
    };
    let (executed, client) = match executed {
        Ok((executed, client)) => {
            if let Some(sink) = sink {
                sink(&case.manifest.id, &executed.snapshots);
            }
            (executed, client)
        }
        Err(error) => {
            let (disposition, error_category) = classify_execution_failure(error.category);
            span.fail(error_category);
            return result(
                case,
                Stage::Execution,
                disposition,
                Some(error.code),
                Some(error.message),
                diagnostics,
                Vec::new(),
                None,
                error.commands,
                error.fuel_consumed,
            );
        }
    };
    // An external-client case passes only when the W3C tester observes the
    // documented response and the receiving session passes the official
    // Outcome oracle, termination included; it is a documented manual
    // equivalent, never an OutcomeLog pass.
    let evaluated = match &client {
        None => oracle::evaluate_for_corpus(corpus_kind, &case.manifest, &executed),
        Some((client, response)) => client
            .evaluate(response, &executed)
            .map(|()| OracleKind::ManualEquivalent),
    };
    match evaluated {
        Ok(oracle_kind) => CaseResult {
            oracle: Some(oracle_kind),
            ..result(
                case,
                Stage::Complete,
                Disposition::Passed,
                None,
                None,
                diagnostics,
                executed.configuration,
                Some(executed.terminal_status),
                executed.commands,
                executed.fuel_consumed,
            )
        },
        Err(failure) => {
            span.fail(ErrorCategory::Oracle);
            CaseResult {
                oracle: Some(if client.is_some() {
                    OracleKind::ManualEquivalent
                } else {
                    oracle::oracle_kind_for_corpus(corpus_kind, &case.manifest)
                }),
                ..result(
                    case,
                    Stage::Oracle,
                    Disposition::Failed,
                    Some(failure.code),
                    Some(failure.message),
                    diagnostics,
                    executed.configuration,
                    Some(executed.terminal_status),
                    executed.commands,
                    executed.fuel_consumed,
                )
            }
        }
    }
}

/// Runs one official case decided by a documented external-client oracle
/// (`crate::external_client`); an oracle whose W3C instruction identity
/// drifted is a preparation `HarnessError`.
fn run_external_client_case<R, B>(
    case: &LoadedCase,
    selected: Result<ExternalClientOracle, external_client::OracleDrift>,
    suite: &LoadedSuite,
    compiler: &Compiler<R, B>,
    executor: &dyn CaseExecutor,
    sink: Option<SnapshotSink<'_>>,
) -> CaseResult
where
    R: scxml_validator_core::SchemaResolver,
    B: scxml_validator_core::SchemaValidator,
{
    match selected {
        Ok(client) => run_case(
            case,
            CaseSource::ExternalClient(client),
            &Suite {
                kind: suite.manifest.suite.kind,
                default_steps: suite.manifest.default_command_budget,
                extensions: &[],
            },
            compiler,
            executor,
            sink,
        ),
        Err(drift) => result(
            case,
            Stage::Preparation,
            Disposition::HarnessError,
            Some(drift.code),
            Some(drift.message),
            Vec::new(),
            Vec::new(),
            None,
            0,
            0,
        ),
    }
}

/// The acquired dependencies of a namespace case below its start document's
/// directory, keyed by their path relative to it.
fn namespace_event_schemas(case: &LoadedCase) -> std::collections::BTreeMap<String, Vec<u8>> {
    let directory = case
        .manifest
        .start_uri
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    case.dependency_sources
        .iter()
        .filter(|(uri, _)| case.manifest.dependency_source_artifacts.contains_key(uri))
        .filter_map(|(uri, bytes)| {
            let relative = if directory.is_empty() {
                Some(uri.as_str())
            } else {
                uri.strip_prefix(directory)
                    .and_then(|rest| rest.strip_prefix('/'))
            };
            relative.map(|relative| (relative.to_owned(), bytes.clone()))
        })
        .collect()
}

/// What every case of one suite run shares.
struct Suite<'a> {
    /// The corpus provenance, which selects the oracle.
    kind: CorpusKind,
    /// The suite's default per-case command budget.
    default_steps: u64,
    /// The extension namespaces the executor binds.
    extensions: &'a [crate::executor::BoundExtension],
}

/// Where a case's executable SCXML comes from.
#[derive(Clone, Copy)]
enum CaseSource {
    /// The prepared W3C artifact.
    Prepared,
    /// The harness-authored receiver of a documented external-client oracle
    /// (`crate::external_client`); the harness is the W3C tester's client.
    ExternalClient(ExternalClientOracle),
}

fn has_validation_errors(report: &scxml_validator_core::Report) -> bool {
    // `Compiler::compile` appends COMP001 only after canonical validation has
    // accepted the document and lowering fails. Keep that later phase out of
    // the validity decision without rerunning either validator or lowerer.
    report
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error && diagnostic.code != "COMP001")
}

fn classify_execution_failure(category: ExecutionFailureCategory) -> (Disposition, ErrorCategory) {
    // Recoverable error.execution events never reach the runner as
    // failures: the machine continues through normal transition selection.
    match category {
        ExecutionFailureCategory::Harness => (Disposition::HarnessError, ErrorCategory::Harness),
        ExecutionFailureCategory::CommandBudget => (Disposition::Failed, ErrorCategory::Budget),
        ExecutionFailureCategory::Unsupported => {
            (Disposition::Unsupported, ErrorCategory::Unsupported)
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "a case result carries every canonical report field explicitly"
)]
fn result(
    case: &LoadedCase,
    stage: Stage,
    disposition: Disposition,
    reason_code: Option<&str>,
    reason: Option<String>,
    diagnostics: Vec<DiagnosticRecord>,
    configuration: Vec<String>,
    terminal_status: Option<crate::report::TerminalStatus>,
    steps: u64,
    fuel_consumed: u64,
) -> CaseResult {
    CaseResult {
        id: case.manifest.id.clone(),
        assertion_id: case.manifest.assertion_id.clone(),
        spec_num: case.manifest.spec_num.clone(),
        spec_id: case.manifest.spec_id.clone(),
        datamodel: case.manifest.datamodel.clone(),
        conformance: case.manifest.conformance,
        manual: case.manifest.manual,
        stage,
        disposition,
        reason_code: reason_code.map(str::to_owned),
        reason,
        diagnostics,
        configuration,
        terminal_status,
        commands: steps,
        fuel_consumed,
        oracle: None,
        erratum: case
            .manifest
            .erratum
            .as_ref()
            .map(|erratum| erratum.id.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::executor::{
        ExecutionError, ExecutionResult, ExecutionTrace, ExecutorIdentities, ExternalClientRun,
        HttpRequest, HttpResponse, LogRecord,
    };
    use crate::report::TerminalStatus;

    /// The audit's edited official case 355: the transition out of `s0` logs
    /// `Outcome` `pass` and enters the non-final state `s1`, where the
    /// session stays running.
    const EDITED_355: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" datamodel="ecmascript" version="1.0">
<state id="s0">
  <transition target="s1"><log label="Outcome" expr="'pass'"/></transition>
</state>
<state id="s1"/>
<final id="pass"><onentry><log label="Outcome" expr="'pass'"/></onentry></final>
<final id="fail"><onentry><log label="Outcome" expr="'fail'"/></onentry></final>
</scxml>"#;

    /// Returns one scripted observation for every run (and, for an
    /// external-client case, one scripted response); stands in for the
    /// component executor so the runner's verdict is tested in isolation.
    struct ScriptedExecutor {
        execution: ExecutionResult,
        response: Option<HttpResponse>,
    }

    impl ScriptedExecutor {
        fn new(execution: ExecutionResult) -> Self {
            Self {
                execution,
                response: None,
            }
        }
    }

    impl CaseExecutor for ScriptedExecutor {
        fn identities(&self) -> ExecutorIdentities {
            let identity = implementation_identity("scripted", "0.0.0", &[b"scripted"]);
            ExecutorIdentities {
                engine: identity.clone(),
                runner: identity.clone(),
                data_models: BTreeMap::new(),
                unrouted_data_model: identity,
            }
        }

        fn execute(
            &self,
            _ir_bytes: &[u8],
            _ir: &MachineIr,
            _events: &[String],
            _max_commands: u64,
            _sources: &BTreeMap<String, String>,
        ) -> Result<ExecutionResult, ExecutionError> {
            Ok(self.execution.clone())
        }

        fn execute_external_client(
            &self,
            _ir_bytes: &[u8],
            _ir: &MachineIr,
            _request: &dyn Fn(&str) -> HttpRequest,
            _max_commands: u64,
            _sources: &BTreeMap<String, String>,
        ) -> Result<ExternalClientRun, ExecutionError> {
            self.response
                .clone()
                .map(|response| ExternalClientRun {
                    execution: self.execution.clone(),
                    response,
                })
                .ok_or_else(|| {
                    ExecutionError::new(
                        ExecutionFailureCategory::Harness,
                        "scripted-no-client",
                        "the scripted executor has no external-client response",
                    )
                })
        }
    }

    fn official_case(id: &str) -> LoadedSuite {
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/conformance/w3c-irp/prepared/ecmascript");
        let mut suite =
            crate::manifest::load_suite(&corpus).expect("the checked-in ECMAScript corpus loads");
        suite.cases.retain(|case| case.manifest.id == id);
        suite
    }

    fn official_355(source: &str) -> LoadedSuite {
        let mut suite = official_case("355");
        let case = suite
            .cases
            .first_mut()
            .expect("official case 355 is in the prepared corpus");
        case.source = Some(source.to_owned());
        suite
    }

    fn observed(status: TerminalStatus, configuration: &str) -> ExecutionResult {
        ExecutionResult {
            configuration: vec![configuration.to_owned()],
            terminal_status: status,
            commands: 1,
            fuel_consumed: 0,
            logs: vec![LogRecord {
                label: "Outcome".into(),
                value: "pass".into(),
            }],
            trace: ExecutionTrace::default(),
            load_rejection: None,
            snapshots: Vec::new(),
        }
    }

    #[test]
    fn an_outcome_pass_that_keeps_running_is_not_passed() {
        let suite = official_355(EDITED_355);
        let executor = ScriptedExecutor::new(observed(TerminalStatus::Running, "s1"));
        let report = run_suite(&suite, &RunOptions::default(), &executor)
            .expect("the report identity encodes");
        let case = &report.cases[0];
        assert_eq!(case.id, "355");
        assert_eq!(case.disposition, Disposition::Failed);
        assert_eq!(case.stage, Stage::Oracle);
        assert_eq!(case.reason_code.as_deref(), Some("outcome-pass-not-final"));
        assert_eq!(case.oracle, Some(OracleKind::OutcomeLog));
        assert_eq!(case.terminal_status, Some(TerminalStatus::Running));
        assert_eq!(case.configuration, ["s1"]);
        assert_eq!(report.summary.passed, 0);
        assert_eq!(report.summary.not_passing, 1);
    }

    #[test]
    fn an_outcome_pass_finished_in_the_pass_final_state_is_passed() {
        let suite = official_355(EDITED_355);
        let executor = ScriptedExecutor::new(observed(TerminalStatus::Finished, "pass"));
        let report = run_suite(&suite, &RunOptions::default(), &executor)
            .expect("the report identity encodes");
        assert_eq!(report.cases[0].disposition, Disposition::Passed);
        assert_eq!(report.cases[0].oracle, Some(OracleKind::OutcomeLog));
    }

    /// Manual case 513: the external client received HTTP 200 and the
    /// receiving session logged `Outcome` `pass`, but the session did not
    /// finish in exactly the `pass` final state.
    #[test]
    fn external_client_513_requires_the_receiver_to_finish_in_pass() {
        let suite = official_case("513");
        let ok = HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: Vec::new(),
        };
        for (status, configuration) in [
            (TerminalStatus::Running, "s0"),
            (TerminalStatus::Finished, "fail"),
            (TerminalStatus::Running, "pass"),
        ] {
            let executor = ScriptedExecutor {
                execution: observed(status, configuration),
                response: Some(ok.clone()),
            };
            let report = run_suite(&suite, &RunOptions::default(), &executor)
                .expect("the report identity encodes");
            let case = &report.cases[0];
            assert_eq!(case.id, "513");
            assert_eq!(
                case.disposition,
                Disposition::Failed,
                "{status:?} {configuration}"
            );
            assert_eq!(case.reason_code.as_deref(), Some("outcome-pass-not-final"));
            assert_eq!(case.oracle, Some(OracleKind::ManualEquivalent));
        }

        let passing = ScriptedExecutor {
            execution: observed(TerminalStatus::Finished, "pass"),
            response: Some(ok.clone()),
        };
        let report = run_suite(&suite, &RunOptions::default(), &passing)
            .expect("the report identity encodes");
        assert_eq!(report.cases[0].disposition, Disposition::Passed);
        assert_eq!(report.cases[0].oracle, Some(OracleKind::ManualEquivalent));

        let refused = ScriptedExecutor {
            execution: observed(TerminalStatus::Finished, "pass"),
            response: Some(HttpResponse { status: 204, ..ok }),
        };
        let report = run_suite(&suite, &RunOptions::default(), &refused)
            .expect("the report identity encodes");
        assert_eq!(
            report.cases[0].reason_code.as_deref(),
            Some("external-client-status-mismatch")
        );
    }

    #[test]
    fn typed_execution_failures_map_to_truthful_dispositions() {
        assert_eq!(
            classify_execution_failure(ExecutionFailureCategory::Unsupported),
            (Disposition::Unsupported, ErrorCategory::Unsupported)
        );
        assert_eq!(
            classify_execution_failure(ExecutionFailureCategory::CommandBudget),
            (Disposition::Failed, ErrorCategory::Budget)
        );
        assert_eq!(
            classify_execution_failure(ExecutionFailureCategory::Harness),
            (Disposition::HarnessError, ErrorCategory::Harness)
        );
    }
}
