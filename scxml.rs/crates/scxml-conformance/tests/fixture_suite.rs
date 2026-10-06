#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

use std::fs;
use std::path::{Path, PathBuf};

use scxml_conformance::manifest::{
    MANIFEST_VERSION, MAX_CASE_FILE_BYTES, MAX_MANIFEST_STRING_BYTES, MAX_NESTED_ITEMS,
    MAX_SUITE_MANIFEST_BYTES,
};
use scxml_conformance::{
    CaseResult, ConformanceLevel, CorpusKind, Disposition, ExpectedOutcome, Filters, LogOutcome,
    RunReport, SuiteIdentity, SuiteManifest, Summary, load_suite, report_identity_digest,
};
#[cfg(feature = "wasmtime")]
use scxml_conformance::{
    IdRange, IdentityPolicy, LoadedCase, LoadedSuite, RunOptions, compare_reports,
};

#[cfg(feature = "wasmtime")]
mod support;

fn manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/corpus.json")
}

fn implementation_identity(
    name: &str,
    digest: char,
) -> scxml_reporting::ConformanceImplementationIdentity {
    scxml_reporting::ConformanceImplementationIdentity {
        name: name.into(),
        version: "0.1.0".into(),
        source_digest: digest.to_string().repeat(64),
    }
}

#[cfg(feature = "wasmtime")]
#[test]
fn synthetic_fixture_suite_classifies_without_claiming_irp_passes() {
    let report = support::run_corpus(&manifest(), RunOptions::default());
    assert_eq!(
        report.suite.kind,
        scxml_conformance::CorpusKind::SyntheticHarness
    );
    let dispositions: Vec<_> = report
        .cases
        .iter()
        .map(|case| {
            (
                case.id.as_str(),
                case.disposition,
                case.reason_code.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        dispositions,
        vec![
            ("001", Disposition::Passed, None),
            ("002", Disposition::Failed, Some("configuration-mismatch")),
            (
                "003",
                Disposition::Unsupported,
                Some("unsupported-data-initializer")
            ),
            ("004", Disposition::Passed, None),
            ("005", Disposition::Passed, None),
            ("006", Disposition::Invalid, Some("source-invalid")),
            ("007", Disposition::Passed, None),
            ("008", Disposition::Passed, None),
        ]
    );
    assert_eq!(report.summary.passed, 5);
    assert_eq!(report.summary.applicable_selected, 8);
    assert_eq!(report.summary.not_passing, 3);
    assert_eq!(report.summary.mandatory.not_passing, 3);
    assert_eq!(report.summary.optional.not_passing, 0);
    let artifact = report
        .canonical_artifact()
        .expect("synthetic report must project without losing producer observations");
    let first = &artifact
        .conformance_payload()
        .expect("synthetic artifact must contain conformance")
        .cases[0];
    assert_eq!(first.spec_num, report.cases[0].spec_num);
    assert_eq!(first.manual, report.cases[0].manual);
    assert_eq!(
        first.terminal_status,
        Some(scxml_reporting::ConformanceTerminalStatus::Finished)
    );
}

#[cfg(feature = "wasmtime")]
#[test]
fn parse_invalid_source_precedes_unsupported_capability_detection() {
    let case = run_case_with_source(
        "004",
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0">
  <parallel id="unsupported">
</scxml>"#,
    );

    assert_eq!(case.disposition, Disposition::Invalid);
    assert_eq!(case.stage, scxml_conformance::Stage::Parse);
    assert_eq!(case.reason_code.as_deref(), Some("source-invalid"));
}

#[cfg(feature = "wasmtime")]
#[test]
fn validation_invalid_source_precedes_unsupported_capability_detection() {
    let case = run_case_with_source(
        "004",
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="parent">
  <state id="parent" initial="child">
    <state id="child">
      <transition target="missing"/>
    </state>
  </state>
</scxml>"#,
    );

    assert_eq!(case.disposition, Disposition::Invalid);
    assert_eq!(case.stage, scxml_conformance::Stage::Validation);
    assert_eq!(case.reason_code.as_deref(), Some("source-invalid"));
}

#[cfg(feature = "wasmtime")]
#[test]
fn invoke_with_unknown_processor_type_fails_at_invocation_start() {
    // Slice-16 semantics: an `<invoke>` whose type names no supported
    // processor raises `error.execution` at invocation start (SCXML 6.4);
    // the machine never reaches its Outcome log, so the oracle reports the
    // official `fail` honestly (no Unsupported shortcut, no Failed hiding).
    let case = run_case_with_source(
        "004",
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="p" datamodel="ecmascript">
  <state id="p">
    <state id="inner"/>
    <invoke type="http://www.w3.org/2005/07/scxml" src="child.scxml"/>
  </state>
</scxml>"#,
    );

    assert_eq!(case.disposition, Disposition::Failed);
    assert_eq!(case.stage, scxml_conformance::Stage::Oracle);
    assert_eq!(case.reason_code.as_deref(), Some("configuration-mismatch"));
}

#[cfg(feature = "wasmtime")]
fn run_case_with_source(case_id: &str, source: &str) -> scxml_conformance::CaseResult {
    let manifest_path = manifest();
    let root = manifest_path
        .parent()
        .expect("synthetic fixture manifest must have a parent")
        .to_path_buf();
    let mut suite_manifest: SuiteManifest = serde_json::from_slice(
        &fs::read(&manifest_path).expect("synthetic fixture manifest must be readable"),
    )
    .expect("synthetic fixture manifest must decode");
    suite_manifest.manifest_version = MANIFEST_VERSION;
    suite_manifest.cases.retain(|case| case.id == case_id);
    assert_eq!(
        suite_manifest.cases.len(),
        1,
        "fixture case must exist once"
    );
    let case_manifest = suite_manifest
        .cases
        .pop()
        .expect("precedence fixture case must remain after exact-id selection");
    let source_path = case_manifest
        .source
        .as_ref()
        .expect("precedence fixture case must name a source artifact");
    let suite = LoadedSuite {
        manifest_path,
        root: root.clone(),
        manifest: suite_manifest,
        cases: vec![LoadedCase {
            source_path: Some(root.join(source_path)),
            source: Some(source.into()),
            manifest: case_manifest,
            dependency_paths: Vec::new(),
            dependency_sources: Vec::new(),
            resource_paths: Vec::new(),
        }],
    };

    let report = support::run(suite, RunOptions::default());
    assert_eq!(
        report.cases.len(),
        1,
        "precedence fixture must produce exactly one result"
    );
    report
        .cases
        .into_iter()
        .next()
        .expect("precedence fixture result must remain after count validation")
}

#[test]
fn synthetic_manifest_cannot_claim_the_official_log_oracle() {
    let mut suite: SuiteManifest = serde_json::from_slice(
        &fs::read(manifest()).expect("synthetic fixture manifest must be readable"),
    )
    .expect("synthetic fixture manifest must decode");
    suite.manifest_version = MANIFEST_VERSION;
    suite.cases[0].expected = ExpectedOutcome::W3cLogOutcome {
        outcome: LogOutcome::Pass,
    };
    let path = std::env::temp_dir().join(format!(
        "scxml-synthetic-official-oracle-{}.json",
        std::process::id()
    ));
    let error = scxml_conformance::manifest::write_suite(&path, &suite)
        .expect_err("synthetic manifests must not claim the official log oracle");
    assert_eq!(error.code, "synthetic-case-oracle");
}

#[cfg(feature = "wasmtime")]
#[test]
fn official_runner_does_not_fall_back_to_synthetic_expectations() {
    let manifest_path = manifest();
    let root = manifest_path
        .parent()
        .expect("synthetic fixture manifest must have a parent")
        .to_path_buf();
    let mut suite_manifest: SuiteManifest = serde_json::from_slice(
        &fs::read(&manifest_path).expect("synthetic fixture manifest must be readable"),
    )
    .expect("synthetic fixture manifest must decode");
    suite_manifest.suite.kind = scxml_conformance::CorpusKind::OfficialIrp;
    suite_manifest.cases.truncate(1);
    suite_manifest.cases[0].expected = ExpectedOutcome::FinalState {
        state: "done".into(),
    };
    let source_path = root.join("cases/pass-final.scxml");
    let source = fs::read_to_string(&source_path).expect("synthetic source must be readable");
    let suite = LoadedSuite {
        manifest_path,
        root,
        manifest: suite_manifest.clone(),
        cases: vec![LoadedCase {
            manifest: suite_manifest.cases[0].clone(),
            source_path: Some(source_path),
            source: Some(source),
            dependency_paths: Vec::new(),
            dependency_sources: Vec::new(),
            resource_paths: Vec::new(),
        }],
    };

    let report = support::run(suite, RunOptions::default());
    assert_eq!(report.cases[0].disposition, Disposition::Failed);
    assert_eq!(
        report.cases[0].reason_code.as_deref(),
        Some("outcome-missing")
    );
}

#[cfg(feature = "wasmtime")]
#[test]
fn filtering_is_deterministic() {
    let filters = Filters {
        id_ranges: vec![IdRange { start: 1, end: 8 }],
        tags: vec!["smoke".into()],
        ..Filters::default()
    };
    let report = support::run_corpus(
        &manifest(),
        RunOptions {
            filters,
            ..RunOptions::default()
        },
    );
    assert_eq!(
        report
            .cases
            .iter()
            .map(|case| case.id.as_str())
            .collect::<Vec<_>>(),
        ["001", "008"]
    );
}

#[test]
fn canonical_reporting_preserves_complete_run_identity() {
    let mut report = RunReport {
        report_version: scxml_reporting::CONFORMANCE_REPORT_VERSION,
        suite: SuiteIdentity {
            errata: None,
            id: "synthetic-conformance-harness".into(),
            version: "1.0.0".into(),
            kind: CorpusKind::SyntheticHarness,
            source: "checked-in synthetic harness fixtures".into(),
            revision: None,
            corpus_digest: "a".repeat(64),
            source_digest: "b".repeat(64),
            manifest_digest: "c".repeat(64),
            template_digests: std::collections::BTreeMap::new(),
            transformer: None,
            datamodel: "mixed".into(),
            provenance: Some("synthetic; never an official IRP result".into()),
        },
        schema_policy: scxml_reporting::ConformanceSchemaPolicy::Strict,
        compiler: implementation_identity("scxml-compiler-core", '1'),
        engine: implementation_identity("scxml-engine", '2'),
        data_model: implementation_identity("scxml-data-model-ecmascript", '3'),
        runner: implementation_identity("scxml-conformance", '4'),
        manual_oracle: implementation_identity("scxml-conformance-manual-oracle", '5'),
        extensions: Vec::new(),
        ir_version: 1,
        filters: Filters {
            ids: vec!["008".into(), "001".into()],
            tags: vec!["smoke".into()],
            ..Filters::default()
        },
        git_commit: Some("0123456789abcdef".into()),
        identity_digest: String::new(),
        summary: Summary::default(),
        cases: Vec::new(),
    };
    report.identity_digest =
        report_identity_digest(&report).expect("fixture report identity must hash");
    let artifact = report
        .canonical_artifact()
        .expect("complete run identity must convert to a canonical artifact");
    let payload = artifact
        .conformance_payload()
        .expect("canonical artifact must retain its conformance payload");

    assert_eq!(payload.identity.suite.id, report.suite.id);
    assert_eq!(payload.identity.suite.manifest_source, report.suite.source);
    assert_eq!(payload.identity.suite.revision, report.suite.revision);
    assert_eq!(
        payload.identity.suite.source_set_digest,
        report.suite.source_digest
    );
    assert_eq!(
        payload.identity.suite.manifest_digest,
        report.suite.manifest_digest
    );
    assert_eq!(
        payload.identity.suite.template_digests,
        report.suite.template_digests
    );
    assert_eq!(payload.identity.schema_policy, report.schema_policy);
    assert_eq!(payload.identity.compiler, report.compiler);
    assert_eq!(payload.identity.engine, report.engine);
    assert_eq!(payload.identity.data_model, report.data_model);
    assert_eq!(payload.identity.runner, report.runner);
    assert_eq!(payload.identity.manual_oracle, report.manual_oracle);
    assert_eq!(payload.identity.ir_version, report.ir_version);
    assert_eq!(payload.identity.filters.ids, ["001", "008"]);
    assert_eq!(payload.identity.filters.tags, ["smoke"]);
    assert_eq!(payload.identity.run_revision, report.git_commit);
    assert_eq!(payload.identity.report_digest, report.identity_digest);

    let first = scxml_reporting::encode_canonical(&artifact)
        .expect("canonical conformance artifact must encode");
    let second = scxml_reporting::encode_canonical(&artifact)
        .expect("canonical conformance artifact must re-encode");
    assert_eq!(first, second);
    assert_eq!(
        scxml_reporting::decode_canonical(&first)
            .expect("canonical conformance artifact must decode"),
        artifact
    );
}

#[cfg(feature = "wasmtime")]
#[test]
fn baseline_identity_mismatch_and_regression_are_explicit() {
    let current = support::run_corpus(&manifest(), RunOptions::default());
    let mut mismatch = current.clone();
    mismatch.suite.corpus_digest = "1".repeat(64);
    assert_eq!(
        compare_reports(&mismatch, &current, IdentityPolicy::Require)
            .unwrap_err()
            .code,
        "baseline-identity-mismatch"
    );

    let mut baseline = current.clone();
    baseline.cases[0].disposition = Disposition::Failed;
    baseline.cases[1].disposition = Disposition::Unsupported;
    baseline.cases[2].disposition = Disposition::Invalid;
    baseline.cases[3].disposition = Disposition::Passed;
    baseline.cases[5].disposition = Disposition::Invalid;
    let comparison = compare_reports(&baseline, &current, IdentityPolicy::Require)
        .expect("compatible fixture reports must compare");
    assert_eq!(comparison.new_passes, ["001"]);
    assert_eq!(comparison.regressions, [] as [&str; 0]);
    assert_eq!(comparison.newly_supported_failing, ["002"]);
    assert_eq!(comparison.newly_unsupported, ["003"] as [&str; 1]);
    assert_eq!(comparison.fixed_invalid, ["003"] as [&str; 1]);
}

#[cfg(feature = "wasmtime")]
#[test]
fn snapshot_round_trips_deterministically() {
    let report = support::run_corpus(&manifest(), RunOptions::default());
    let directory = std::env::temp_dir().join(format!(
        "scxml-conformance-{}-{}",
        std::process::id(),
        report.cases.len()
    ));
    fs::create_dir_all(&directory).expect("snapshot fixture directory must be created");
    let path = directory.join("snapshot.json");
    report
        .write_json(&path)
        .expect("snapshot report must be writable");
    let loaded = RunReport::read_json(&path).expect("snapshot report must be readable");
    assert_eq!(loaded, report);
    assert_eq!(
        report_identity_digest(&loaded).expect("loaded snapshot identity must hash"),
        loaded.identity_digest
    );
    let first = fs::read(&path).expect("first snapshot bytes must be readable");
    report
        .write_json(&path)
        .expect("snapshot report must be rewritable");
    assert_eq!(
        fs::read(&path).expect("rewritten snapshot bytes must be readable"),
        first
    );
    fs::remove_dir_all(directory).expect("snapshot fixture directory must be removable");
}

#[test]
fn execution_stage_unsupported_report_round_trips_canonically() {
    let case = CaseResult {
        erratum: None,
        id: "302".into(),
        assertion_id: "302".into(),
        spec_num: "5.8".into(),
        spec_id: "#script".into(),
        datamodel: "ecmascript".into(),
        conformance: ConformanceLevel::Mandatory,
        manual: false,
        stage: scxml_conformance::Stage::Execution,
        disposition: Disposition::Unsupported,
        reason_code: Some("unsupported-executable-content".into()),
        reason: Some("script evaluation is not supported".into()),
        diagnostics: Vec::new(),
        configuration: Vec::new(),
        terminal_status: None,
        commands: 1,
        fuel_consumed: 1,
        oracle: None,
    };
    let mut report = RunReport {
        report_version: scxml_reporting::CONFORMANCE_REPORT_VERSION,
        suite: SuiteIdentity {
            errata: None,
            id: "synthetic-execution-unsupported".into(),
            version: "1.0.0".into(),
            kind: CorpusKind::SyntheticHarness,
            source: "in-memory contract fixture".into(),
            revision: None,
            corpus_digest: "a".repeat(64),
            source_digest: "b".repeat(64),
            manifest_digest: "c".repeat(64),
            template_digests: std::collections::BTreeMap::new(),
            transformer: None,
            datamodel: "ecmascript".into(),
            provenance: Some("synthetic; never an official IRP result".into()),
        },
        schema_policy: scxml_reporting::ConformanceSchemaPolicy::Strict,
        compiler: implementation_identity("scxml-compiler-core", '1'),
        engine: implementation_identity("scxml-engine", '2'),
        data_model: implementation_identity("scxml-data-model-ecmascript", '3'),
        runner: implementation_identity("scxml-conformance", '4'),
        manual_oracle: implementation_identity("scxml-conformance-manual-oracle", '5'),
        extensions: Vec::new(),
        ir_version: 1,
        filters: Filters::default(),
        git_commit: None,
        identity_digest: String::new(),
        summary: Summary::from_cases(std::slice::from_ref(&case)),
        cases: vec![case],
    };
    report.identity_digest =
        report_identity_digest(&report).expect("execution-stage report identity must hash");

    let directory = report_test_directory("execution-stage-unsupported");
    fs::create_dir_all(&directory).expect("execution-stage unsupported directory must be created");
    let path = directory.join("report.json");

    let mut inconsistent = report.clone();
    inconsistent.cases[0].terminal_status = Some(scxml_conformance::TerminalStatus::Running);
    inconsistent.identity_digest = report_identity_digest(&inconsistent)
        .expect("inconsistent execution-stage report identity must hash");
    assert_eq!(
        inconsistent.write_json(&path).unwrap_err().code,
        "report-case-invariant"
    );

    report
        .write_json(&path)
        .expect("execution-stage unsupported report must be writable");
    let first = fs::read(&path).expect("first canonical report bytes must be readable");
    let decoded = RunReport::read_json(&path)
        .expect("execution-stage unsupported report must validate and decode");
    assert_eq!(decoded, report);
    decoded
        .write_json(&path)
        .expect("decoded execution-stage unsupported report must be rewritable");
    assert_eq!(
        fs::read(&path).expect("rewritten canonical report bytes must be readable"),
        first
    );
    let artifact = decoded
        .canonical_artifact()
        .expect("execution-stage unsupported report must convert to canonical artifact");
    let encoded = scxml_reporting::encode_canonical(&artifact)
        .expect("execution-stage unsupported artifact must encode");
    assert_eq!(
        scxml_reporting::decode_canonical(&encoded)
            .expect("execution-stage unsupported artifact must decode"),
        artifact
    );
    fs::remove_dir_all(directory).expect("execution-stage unsupported directory must be removable");
}

#[cfg(feature = "wasmtime")]
#[test]
fn report_reader_rejects_noncanonical_and_inconsistent_baselines() {
    let report = support::run_corpus(&manifest(), RunOptions::default());
    let directory = report_test_directory("invalid-baseline");
    fs::create_dir_all(&directory).expect("invalid-baseline directory must be created");
    let path = directory.join("snapshot.json");

    fs::write(
        &path,
        serde_json::to_vec(&report).expect("report fixture must serialize"),
    )
    .expect("noncanonical report fixture must be writable");
    assert_eq!(
        RunReport::read_json(&path).unwrap_err().code,
        "report-noncanonical"
    );

    let mut value = serde_json::to_value(&report).expect("report fixture must become JSON");
    value["summary"]["passed"] = serde_json::json!(999);
    write_pretty_json(&path, &value);
    assert_eq!(
        RunReport::read_json(&path).unwrap_err().code,
        "report-summary"
    );

    let mut value = serde_json::to_value(&report).expect("report fixture must become JSON");
    value["report_version"] = serde_json::json!(999);
    write_pretty_json(&path, &value);
    assert_eq!(
        RunReport::read_json(&path).unwrap_err().code,
        "report-version"
    );

    let mut value = serde_json::to_value(&report).expect("report fixture must become JSON");
    value["identity_digest"] = serde_json::json!("0".repeat(64));
    write_pretty_json(&path, &value);
    assert_eq!(
        RunReport::read_json(&path).unwrap_err().code,
        "report-identity-tampered"
    );

    fs::remove_dir_all(directory).expect("invalid-baseline directory must be removable");
}

#[cfg(feature = "wasmtime")]
#[test]
fn report_validation_rejects_duplicate_ids_and_case_invariant_tampering() {
    let report = support::run_corpus(&manifest(), RunOptions::default());
    let directory = report_test_directory("case-invariants");
    fs::create_dir_all(&directory).expect("case-invariant directory must be created");
    let path = directory.join("snapshot.json");

    let mut duplicate = report.clone();
    duplicate.cases[1].id = duplicate.cases[0].id.clone();
    duplicate.summary = scxml_conformance::Summary::from_cases(&duplicate.cases);
    assert_eq!(
        duplicate.write_json(&path).unwrap_err().code,
        "report-duplicate-case-id"
    );

    let mut invalid = report.clone();
    invalid.cases[0].stage = scxml_conformance::Stage::Oracle;
    invalid.summary = scxml_conformance::Summary::from_cases(&invalid.cases);
    assert_eq!(
        invalid.write_json(&path).unwrap_err().code,
        "report-case-invariant"
    );

    let mut invalid = report.clone();
    invalid.cases[1].reason_code = None;
    assert_eq!(
        invalid.write_json(&path).unwrap_err().code,
        "report-case-invariant"
    );

    let mut out_of_order = report.clone();
    out_of_order.cases.swap(0, 1);
    assert_eq!(
        out_of_order.write_json(&path).unwrap_err().code,
        "report-case-order"
    );

    fs::remove_dir_all(directory).expect("case-invariant directory must be removable");
}

#[cfg(feature = "wasmtime")]
#[test]
fn report_validation_rejects_oversized_strings_and_collections() {
    let report = support::run_corpus(&manifest(), RunOptions::default());
    let directory = report_test_directory("bounds");
    fs::create_dir_all(&directory).expect("bounds fixture directory must be created");
    let path = directory.join("snapshot.json");

    let mut oversized_string = report.clone();
    oversized_string.cases[0].reason = Some("x".repeat(64 * 1024 + 1));
    assert_eq!(
        oversized_string.write_json(&path).unwrap_err().code,
        "report-string"
    );

    let mut oversized_collection = report.clone();
    oversized_collection.filters.ids = vec!["001".into(); 100_001];
    assert_eq!(
        oversized_collection.write_json(&path).unwrap_err().code,
        "report-collection"
    );

    let oversized_path = directory.join("oversized.json");
    let oversized =
        fs::File::create(&oversized_path).expect("oversized report fixture must be creatable");
    oversized
        .set_len(scxml_reporting::MAX_CANONICAL_BYTES as u64 + 1)
        .expect("oversized report fixture length must be settable");
    assert_eq!(
        RunReport::read_json(&oversized_path).unwrap_err().code,
        "report-size"
    );

    fs::remove_dir_all(directory).expect("bounds fixture directory must be removable");
}

#[test]
fn prepared_corpus_rejects_tampered_artifacts() {
    let directory = std::env::temp_dir().join(format!("scxml-tamper-{}", std::process::id()));
    copy_directory(manifest().parent().expect("manifest parent"), &directory);
    fs::write(directory.join("cases/pass-final.scxml"), "<tampered/>")
        .expect("tampered fixture must be writable");
    assert_eq!(
        load_suite(&directory.join("corpus.json")).unwrap_err().code,
        "artifact-tampered"
    );
    fs::remove_dir_all(directory).expect("tampered fixture directory must be removable");
}

#[test]
fn suite_loader_rejects_over_bound_manifest_and_structure() {
    let oversized = temporary_directory("scxml-manifest-size");
    fs::create_dir_all(&oversized).expect("oversized-manifest directory must be created");
    let oversized_manifest = oversized.join("corpus.json");
    fs::File::create(&oversized_manifest)
        .expect("oversized manifest file must be created")
        .set_len(
            u64::try_from(MAX_SUITE_MANIFEST_BYTES + 1)
                .expect("the manifest byte bound plus one must fit in u64"),
        )
        .expect("oversized manifest must be extended past the byte bound");
    assert_eq!(
        load_suite(&oversized_manifest).unwrap_err().code,
        "manifest-size"
    );
    fs::remove_dir_all(oversized).expect("oversized-manifest directory must be removable");

    let string_bound = fixture_copy("scxml-manifest-string");
    let mut corpus = read_fixture_manifest(&string_bound);
    corpus["suite"]["id"] = serde_json::Value::String("x".repeat(MAX_MANIFEST_STRING_BYTES + 1));
    write_fixture_manifest(&string_bound, &corpus);
    assert_eq!(
        load_suite(&string_bound.join("corpus.json"))
            .unwrap_err()
            .code,
        "manifest-string-size"
    );
    fs::remove_dir_all(string_bound).expect("string-bound fixture directory must be removable");

    let collection_bound = fixture_copy("scxml-manifest-collection");
    let mut corpus = read_fixture_manifest(&collection_bound);
    corpus["cases"][0]["tags"] = serde_json::Value::Array(
        (0..=MAX_NESTED_ITEMS)
            .map(|_| serde_json::Value::String("bounded".into()))
            .collect(),
    );
    write_fixture_manifest(&collection_bound, &corpus);
    assert_eq!(
        load_suite(&collection_bound.join("corpus.json"))
            .unwrap_err()
            .code,
        "manifest-count"
    );
    fs::remove_dir_all(collection_bound)
        .expect("collection-bound fixture directory must be removable");
}

#[test]
fn suite_loader_bounds_source_dependency_and_resource_reads() {
    for kind in ["source", "dependency", "resource"] {
        let directory = fixture_copy(&format!("scxml-{kind}-size"));
        let relative = match kind {
            "source" => PathBuf::from("cases/pass-final.scxml"),
            "dependency" => PathBuf::from("dependencies/oversized.xml"),
            "resource" => PathBuf::from("resources/oversized.txt"),
            _ => unreachable!("test exhaustively names bounded file kinds"),
        };
        if let Some(parent) = directory.join(&relative).parent() {
            fs::create_dir_all(parent).expect("oversized case file parent must be created");
        }
        fs::File::create(directory.join(&relative))
            .expect("oversized case file must be created")
            .set_len(
                u64::try_from(MAX_CASE_FILE_BYTES + 1)
                    .expect("the case file byte bound plus one must fit in u64"),
            )
            .expect("oversized case file must be extended past the byte bound");

        if kind != "source" {
            let mut corpus = read_fixture_manifest(&directory);
            let relative = relative.to_string_lossy().into_owned();
            if kind == "dependency" {
                corpus["cases"][0]["dependencies"] = serde_json::json!([relative.clone()]);
                corpus["cases"][0]["dependency_digests"] =
                    serde_json::json!({ relative: "0".repeat(64) });
            } else {
                corpus["cases"][0]["resources"] = serde_json::json!([relative.clone()]);
                corpus["cases"][0]["resource_digests"] =
                    serde_json::json!({ relative: "0".repeat(64) });
            }
            write_fixture_manifest(&directory, &corpus);
        }

        assert_eq!(
            load_suite(&directory.join("corpus.json")).unwrap_err().code,
            "case-file-size",
            "{kind} reads must enforce the case file byte limit"
        );
        fs::remove_dir_all(directory).expect("oversized case fixture directory must be removable");
    }
}

fn temporary_directory(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{name}-{}", std::process::id()))
}

fn fixture_copy(name: &str) -> PathBuf {
    let directory = temporary_directory(name);
    copy_directory(manifest().parent().expect("manifest parent"), &directory);
    directory
}

fn read_fixture_manifest(directory: &Path) -> serde_json::Value {
    let bytes =
        fs::read(directory.join("corpus.json")).expect("fixture corpus.json must be readable");
    serde_json::from_slice(&bytes).expect("fixture corpus.json must be valid JSON")
}

fn write_fixture_manifest(directory: &Path, manifest: &serde_json::Value) {
    fs::write(
        directory.join("corpus.json"),
        serde_json::to_vec_pretty(manifest).expect("fixture manifest must serialize to JSON"),
    )
    .expect("fixture corpus.json must be writable");
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("fixture destination directory must be created");
    for entry in fs::read_dir(source).expect("fixture source directory must be readable") {
        let entry = entry.expect("fixture source entry must be readable");
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .expect("fixture source entry type must be readable")
            .is_dir()
        {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("fixture file must be copied");
        }
    }
}

fn report_test_directory(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("scxml-conformance-{label}-{}", std::process::id()))
}

#[cfg(feature = "wasmtime")]
fn write_pretty_json(path: &Path, value: &serde_json::Value) {
    let mut bytes =
        serde_json::to_vec_pretty(value).expect("report JSON fixture must serialize prettily");
    bytes.push(b'\n');
    fs::write(path, bytes).expect("pretty report JSON fixture must be writable");
}
