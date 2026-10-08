//! Comparison of a fresh conformance run with a checked-in canonical
//! baseline (`tests/conformance/w3c-irp/baselines/*.scxml-report.json`).
//!
//! # Contract
//!
//! * Preconditions: `baseline` is untrusted bytes; they are decoded and
//!   validated as a canonical conformance artifact before any comparison.
//! * Postconditions: [`compare_to_baseline`] returns `Ok(Ok(()))` exactly
//!   when the fresh run agrees with the baseline on every compared field;
//!   otherwise `Ok(Err(_))` carries every mismatch, sorted by
//!   `(case, field)` with report-level mismatches (`case: None`) first, so
//!   identical inputs always produce identical output.
//! * Compared: `report_version`, the suite identity, schema policy, IR
//!   version, filters, summary, the ordered case-id list, and every field of
//!   every canonical case record.
//! * Not compared: the implementation identities (compiler, engine, data
//!   model, runner, manual oracle), the run revision, and the report digest;
//!   they name the producing toolchain.
//! * Side effects: none; the comparison is pure and bounded by the sizes
//!   the canonical decoder already bounds.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

use crate::report::RunReport;

/// One field on which a fresh run disagrees with its baseline.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BaselineMismatch {
    /// Case identifier, or `None` for a report-level field.
    pub case: Option<String>,
    /// Stable name of the disagreeing field.
    pub field: &'static str,
    /// Canonical JSON rendering of the baseline's value.
    pub baseline: String,
    /// Canonical JSON rendering of the fresh run's value.
    pub fresh: String,
}

impl fmt::Display for BaselineMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.case {
            Some(case) => write!(formatter, "case {case} {}", self.field)?,
            None => write!(formatter, "report {}", self.field)?,
        }
        write!(
            formatter,
            ": baseline={} fresh={}",
            self.baseline, self.fresh
        )
    }
}

/// Failure to read a checked-in canonical baseline or to encode the fresh
/// report for comparison.
#[derive(Debug)]
pub struct BaselineError(pub scxml_reporting::ArtifactError);

impl fmt::Display for BaselineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "baseline comparison failed: {}", self.0)
    }
}

impl std::error::Error for BaselineError {}

/// Compares a fresh run against a checked-in canonical baseline
/// (`*.scxml-report.json`): the suite identity, schema policy, IR version,
/// filters, summary, ordered case ids, and every field of every case
/// record (disposition, reason, oracle, erratum, not-applicable
/// declarations, diagnostics, configuration, terminal status, commands, and
/// fuel). The implementation identities (compiler, engine, data model,
/// runner, manual oracle), run revision, and report digest name the
/// producing toolchain and are not compared.
///
/// # Errors
///
/// [`BaselineError`] when the baseline bytes do not decode as a validated
/// conformance artifact or the fresh report cannot be encoded; otherwise
/// the inner result carries every mismatch, sorted.
pub fn compare_to_baseline(
    report: &RunReport,
    baseline: &[u8],
) -> Result<Result<(), Vec<BaselineMismatch>>, BaselineError> {
    let decoded = scxml_reporting::decode_canonical(baseline).map_err(BaselineError)?;
    let fresh = report.canonical_artifact().map_err(BaselineError)?;
    let (Some(expected), Some(actual)) =
        (decoded.conformance_payload(), fresh.conformance_payload())
    else {
        return Err(BaselineError(scxml_reporting::ArtifactError::InvalidJson(
            "baseline is not a conformance artifact".into(),
        )));
    };
    let mut mismatches = Vec::new();
    let identity = [
        (
            "report_version",
            render(&expected.report_version),
            render(&actual.report_version),
        ),
        (
            "suite",
            render(&expected.identity.suite),
            render(&actual.identity.suite),
        ),
        (
            "schema_policy",
            render(&expected.identity.schema_policy),
            render(&actual.identity.schema_policy),
        ),
        (
            "ir_version",
            render(&expected.identity.ir_version),
            render(&actual.identity.ir_version),
        ),
        (
            "filters",
            render(&expected.identity.filters),
            render(&actual.identity.filters),
        ),
        (
            "summary",
            render(&expected.summary),
            render(&actual.summary),
        ),
        (
            "cases",
            render(&baseline_case_ids(&expected.cases)),
            render(&baseline_case_ids(&actual.cases)),
        ),
    ];
    for (name, baseline, fresh) in identity {
        push_mismatch(&mut mismatches, None, name, baseline, fresh);
    }
    let actual_cases = actual
        .cases
        .iter()
        .map(|case| (case.id.as_str(), case))
        .collect::<BTreeMap<_, _>>();
    for expected_case in &expected.cases {
        if let Some(actual_case) = actual_cases.get(expected_case.id.as_str()) {
            compare_case_record(expected_case, actual_case, &mut mismatches);
        }
    }
    mismatches.sort();
    mismatches.dedup();
    Ok(if mismatches.is_empty() {
        Ok(())
    } else {
        Err(mismatches)
    })
}

fn baseline_case_ids(cases: &[scxml_reporting::ConformanceCase]) -> Vec<&str> {
    cases.iter().map(|case| case.id.as_str()).collect()
}

fn push_mismatch(
    mismatches: &mut Vec<BaselineMismatch>,
    case: Option<&str>,
    field: &'static str,
    baseline: String,
    fresh: String,
) {
    if baseline != fresh {
        mismatches.push(BaselineMismatch {
            case: case.map(str::to_owned),
            field,
            baseline,
            fresh,
        });
    }
}

/// Compares every field of one canonical case record.
fn compare_case_record(
    expected: &scxml_reporting::ConformanceCase,
    actual: &scxml_reporting::ConformanceCase,
    mismatches: &mut Vec<BaselineMismatch>,
) {
    let (
        Ok(serde_json::Value::Object(expected_fields)),
        Ok(serde_json::Value::Object(actual_fields)),
    ) = (serde_json::to_value(expected), serde_json::to_value(actual))
    else {
        push_mismatch(
            mismatches,
            Some(&expected.id),
            "record",
            render(expected),
            render(actual),
        );
        return;
    };
    for (name, expected_value) in &expected_fields {
        let actual_value = actual_fields.get(name).unwrap_or(&serde_json::Value::Null);
        push_mismatch(
            mismatches,
            Some(&expected.id),
            case_field_name(name),
            render(expected_value),
            render(actual_value),
        );
    }
}

/// Static name of one canonical case-record field.
fn case_field_name(name: &str) -> &'static str {
    match name {
        "id" => "id",
        "assertion_id" => "assertion_id",
        "spec_num" => "spec_num",
        "spec_id" => "spec_id",
        "datamodel" => "datamodel",
        "conformance" => "conformance",
        "manual" => "manual",
        "stage" => "stage",
        "disposition" => "disposition",
        "reason_code" => "reason_code",
        "reason" => "reason",
        "diagnostics" => "diagnostics",
        "configuration" => "configuration",
        "terminal_status" => "terminal_status",
        "commands" => "commands",
        "fuel_consumed" => "fuel_consumed",
        "oracle" => "oracle",
        "erratum" => "erratum",
        _ => "unknown-field",
    }
}

/// Canonical JSON of one compared value. Every compared type is a plain
/// serde data type (strings, integers, enums, vectors, and structs of them),
/// for which `serde_json` serialization cannot fail; the fallback keeps the
/// comparison total and still reports a mismatch against any real value.
fn render<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Filters;
    use crate::manifest::{ConformanceLevel, CorpusKind, SuiteIdentity};
    use crate::report::{
        CaseResult, DiagnosticRecord, Disposition, OracleKind, Stage, Summary, TerminalStatus,
        report_identity_digest,
    };

    /// `(case-level?, field, mutation)`; a case-level mutation edits the
    /// first case and keeps the report internally consistent.
    type Mutation = (bool, &'static str, fn(&mut RunReport));

    fn identity(name: &str, digit: char) -> scxml_reporting::ConformanceImplementationIdentity {
        scxml_reporting::ConformanceImplementationIdentity {
            name: name.into(),
            version: "0.1.0".into(),
            source_digest: digit.to_string().repeat(64),
        }
    }

    fn case(id: &str, disposition: Disposition) -> CaseResult {
        let (stage, reason_code) = match disposition {
            Disposition::Passed => (Stage::Complete, None),
            _ => (Stage::Oracle, Some("configuration-mismatch".to_owned())),
        };
        CaseResult {
            id: id.into(),
            assertion_id: id.into(),
            spec_num: "3.1".into(),
            spec_id: "#core".into(),
            datamodel: "null".into(),
            conformance: ConformanceLevel::Mandatory,
            manual: false,
            stage,
            disposition,
            reason: reason_code.as_ref().map(|_| "mismatch".to_owned()),
            reason_code,
            diagnostics: Vec::new(),
            configuration: vec!["done".into()],
            terminal_status: Some(TerminalStatus::Finished),
            commands: 2,
            fuel_consumed: 2,
            oracle: Some(OracleKind::SyntheticExpectation),
            erratum: None,
        }
    }

    /// A consistent in-memory report of three synthetic cases.
    fn fresh() -> RunReport {
        let cases = vec![
            case("001", Disposition::Passed),
            case("002", Disposition::Failed),
            case("003", Disposition::Passed),
        ];
        let mut report = RunReport {
            report_version: scxml_reporting::CONFORMANCE_REPORT_VERSION,
            suite: SuiteIdentity {
                errata: None,
                id: "synthetic-baseline-comparison".into(),
                version: "1.0.0".into(),
                kind: CorpusKind::SyntheticHarness,
                source: "in-memory contract fixture".into(),
                revision: None,
                corpus_digest: "a".repeat(64),
                source_digest: "b".repeat(64),
                manifest_digest: "c".repeat(64),
                template_digests: BTreeMap::new(),
                transformer: None,
                datamodel: "null".into(),
                provenance: Some("synthetic; never an official IRP result".into()),
            },
            schema_policy: scxml_reporting::ConformanceSchemaPolicy::Off,
            compiler: identity("scxml-compiler-core", '1'),
            engine: identity("scxml-machine", '2'),
            data_model: identity("scxml-data-model-null", '3'),
            runner: identity("scxml-conformance/wasm", '4'),
            manual_oracle: identity("scxml-conformance-manual-oracle", '5'),
            extensions: Vec::new(),
            ir_version: 1,
            filters: Filters::default(),
            git_commit: None,
            identity_digest: String::new(),
            summary: Summary::from_cases(&cases),
            cases,
        };
        seal(&mut report);
        report
    }

    fn seal(report: &mut RunReport) {
        report.summary = Summary::from_cases(&report.cases);
        report.identity_digest = report_identity_digest(report).expect("identity must encode");
    }

    fn baseline_bytes(report: &RunReport) -> Vec<u8> {
        scxml_reporting::encode_canonical(
            &report
                .canonical_artifact()
                .expect("fixture must be canonical"),
        )
        .expect("fixture must encode")
    }

    fn mismatches(report: &RunReport, baseline: &[u8]) -> Vec<BaselineMismatch> {
        compare_to_baseline(report, baseline)
            .expect("baseline must decode")
            .expect_err("a divergent run must list its mismatches")
    }

    #[test]
    fn a_run_reproduces_its_own_baseline_whatever_its_toolchain_identities() {
        let report = fresh();
        let baseline = baseline_bytes(&report);
        assert_eq!(
            compare_to_baseline(&report, &baseline).expect("decodes"),
            Ok(())
        );

        let mut other_toolchain = report;
        other_toolchain.compiler.source_digest = "6".repeat(64);
        other_toolchain.engine.name = "another-engine".into();
        other_toolchain.data_model.source_digest = "7".repeat(64);
        other_toolchain.runner.source_digest = "8".repeat(64);
        other_toolchain.manual_oracle.version = "9".into();
        other_toolchain.git_commit = Some("0123456789abcdef".into());
        seal(&mut other_toolchain);
        assert_eq!(
            compare_to_baseline(&other_toolchain, &baseline).expect("decodes"),
            Ok(())
        );
    }

    #[test]
    fn every_compared_field_divergence_is_named() {
        let baseline = baseline_bytes(&fresh());
        let mutations: &[Mutation] = &[
            (false, "suite", |r| r.suite.revision = Some("r2".into())),
            (false, "schema_policy", |r| {
                r.schema_policy = scxml_reporting::ConformanceSchemaPolicy::Strict;
            }),
            (false, "ir_version", |r| r.ir_version += 1),
            (false, "filters", |r| r.filters.ids.push("001".into())),
            (true, "assertion_id", |r| r.cases[0].assertion_id.push('x')),
            (true, "spec_num", |r| r.cases[0].spec_num.push('x')),
            (true, "spec_id", |r| r.cases[0].spec_id.push('x')),
            (true, "manual", |r| r.cases[0].manual = true),
            (true, "diagnostics", |r| {
                r.cases[0].diagnostics.push(DiagnosticRecord {
                    code: "SCXML999".into(),
                    severity: "warning".into(),
                    message: "x".into(),
                });
            }),
            (true, "configuration", |r| {
                r.cases[0].configuration.push("elsewhere".into());
            }),
            (true, "terminal_status", |r| {
                r.cases[0].terminal_status = Some(TerminalStatus::Running);
            }),
            (true, "commands", |r| r.cases[0].commands += 1),
            (true, "fuel_consumed", |r| r.cases[0].fuel_consumed += 1),
            (true, "disposition", |r| {
                r.cases[0] = CaseResult {
                    id: "001".into(),
                    assertion_id: "001".into(),
                    ..case("001", Disposition::Failed)
                };
            }),
            (true, "reason", |r| r.cases[1].reason = Some("other".into())),
        ];
        for (case_level, field, mutate) in mutations {
            let mut report = fresh();
            mutate(&mut report);
            seal(&mut report);
            let found = mismatches(&report, &baseline);
            assert!(
                found.iter().any(|mismatch| mismatch.field == *field
                    && mismatch.case.is_some() == *case_level
                    && mismatch.baseline != mismatch.fresh),
                "{field}: {found:?}"
            );
        }
    }

    #[test]
    fn missing_extra_and_reordered_cases_are_detected_and_sorted() {
        let baseline = baseline_bytes(&fresh());

        let mut missing = fresh();
        missing.cases.pop();
        seal(&mut missing);
        let found = mismatches(&missing, &baseline);
        assert!(found.iter().any(|m| m.case.is_none() && m.field == "cases"));

        let mut reordered = fresh();
        reordered.cases.swap(0, 2);
        seal(&mut reordered);
        assert!(
            compare_to_baseline(&reordered, &baseline).is_err(),
            "a run in noncanonical case order fails closed"
        );

        let mut extra = fresh();
        extra.cases.push(case("004", Disposition::Passed));
        seal(&mut extra);
        let found = mismatches(&extra, &baseline);
        assert!(found.iter().any(|m| m.case.is_none() && m.field == "cases"));
        assert!(
            found
                .iter()
                .any(|m| m.case.is_none() && m.field == "summary")
        );

        let mut several = fresh();
        several.cases[2].commands += 1;
        several.cases[0].configuration.clear();
        several.ir_version += 1;
        seal(&mut several);
        let found = mismatches(&several, &baseline);
        let mut sorted = found.clone();
        sorted.sort();
        assert_eq!(found, sorted, "report-level first, then by case and field");
        assert_eq!(found[0].case, None);
        assert_eq!(found[0].field, "ir_version");
        assert_eq!(
            found.iter().map(|m| m.case.as_deref()).collect::<Vec<_>>(),
            [None, Some("001"), Some("003")]
        );
    }

    #[test]
    fn undecodable_baseline_bytes_fail_closed() {
        assert!(compare_to_baseline(&fresh(), b"not a report").is_err());
    }
}
