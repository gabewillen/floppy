//! The checked-in W3C IRP corpora (ECMAScript and `XPath`), and every
//! repository namespace's own suite, run entirely
//! through the shipped Wasm components under the Wasmtime session host and
//! must reproduce the checked-in canonical baselines byte for byte:
//! identity, dispositions, reasons, oracles, errata, not-applicable
//! declarations, diagnostics, configurations, terminal status, steps, and
//! fuel. On divergence the per-field mismatches are listed first.
//!
//! Requires the `wasmtime` feature; the release components come from
//! `tests/support` (built and verified by
//! `scripts/verify-wasm-capabilities.sh`, or `SCXML_COMPONENTS_DIR` and
//! `SCXML_COMPONENTS_MANIFEST_SHA256`). Run it in release mode:
//! `cargo test --release -p scxml-conformance --features wasmtime --test irp_components`.
//! `SCXML_IRP_DATAMODEL=ecmascript|xpath` limits the corpus run to one data
//! model.
#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use scxml_conformance::manual_oracle::{MANUAL_ORACLE_NAME, MANUAL_ORACLE_VERSION, ManualOracle};
use scxml_conformance::{
    Disposition, Filters, OracleKind, RunOptions, RunReport, Stage, compare_to_baseline,
};

fn irp_root() -> PathBuf {
    support::workspace().join("tests/conformance/w3c-irp")
}

fn selected_data_models() -> Vec<&'static str> {
    let only = std::env::var("SCXML_IRP_DATAMODEL").ok();
    ["ecmascript", "xpath"]
        .into_iter()
        .filter(|datamodel| only.as_deref().is_none_or(|only| only == *datamodel))
        .collect()
}

#[test]
fn checked_in_irp_corpora_reproduce_their_baselines_through_components() {
    let root = irp_root();
    let mut failures = Vec::new();
    for datamodel in selected_data_models() {
        let report = support::run_corpus(
            &root.join("prepared").join(datamodel),
            RunOptions::default(),
        );
        let baseline = std::fs::read(
            root.join("baselines")
                .join(format!("{datamodel}.scxml-report.json")),
        )
        .expect("the checked-in baseline reads");
        match compare_to_baseline(&report, &baseline).expect("the baseline decodes") {
            Ok(()) => eprintln!("{datamodel}: {}", report.summary.concise()),
            Err(mismatches) => {
                eprintln!(
                    "{datamodel}: {} ({} mismatches)",
                    report.summary.concise(),
                    mismatches.len()
                );
                for mismatch in &mismatches {
                    eprintln!("  {datamodel} {mismatch}");
                }
                failures.push((datamodel, mismatches.len()));
                continue;
            }
        }
        assert_measured_baseline(datamodel, &report);
        // Byte identity also binds the component runner, machine, and
        // data-model identities and the report digest.
        let fresh = scxml_reporting::encode_canonical(
            &report
                .canonical_artifact()
                .expect("the component run is a canonical artifact"),
        )
        .expect("the canonical artifact encodes");
        assert!(
            fresh == baseline,
            "{datamodel}: the component run reproduces every case but not the baseline's \
             identity bytes; regenerate the baseline with \
             `cargo run --release -p scxml-conformance --features wasmtime \
             --example generate_irp_baselines -- --components DIR \
             --components-manifest-sha256 HEX --extension-component-sha256 HEX`"
        );
    }
    assert!(
        failures.is_empty(),
        "component runs diverge from the checked-in baselines: {failures:?}"
    );
}

/// Every repository namespace's own suite (`namespaces/<name>/irp`) runs
/// through the same components, binding the packages it uses, and must
/// reproduce its checked-in baseline byte for byte with every case passing.
#[test]
fn checked_in_namespace_suites_reproduce_their_baselines_through_components() {
    let mut failures = Vec::new();
    for (name, _) in scxml_conformance::NAMESPACE_SUITES {
        for datamodel in selected_data_models() {
            let report = support::run_namespace_corpus(name, datamodel);
            let baseline = std::fs::read(
                support::workspace()
                    .join("namespaces")
                    .join(name)
                    .join("irp/baselines")
                    .join(format!("{datamodel}.scxml-report.json")),
            )
            .expect("the checked-in namespace baseline reads");
            match compare_to_baseline(&report, &baseline).expect("the baseline decodes") {
                Ok(()) => eprintln!("{name} {datamodel}: {}", report.summary.concise()),
                Err(mismatches) => {
                    for mismatch in &mismatches {
                        eprintln!("  {name} {datamodel} {mismatch}");
                    }
                    failures.push((name, datamodel, mismatches.len()));
                    continue;
                }
            }
            assert_eq!(
                (report.summary.passed, report.summary.not_passing),
                (report.summary.applicable_selected, 0),
                "{name} {datamodel}: every namespace case passes"
            );
            let fresh = scxml_reporting::encode_canonical(
                &report
                    .canonical_artifact()
                    .expect("the namespace run is a canonical artifact"),
            )
            .expect("the canonical artifact encodes");
            assert!(
                fresh == baseline,
                "{name} {datamodel}: the component run reproduces every case but not the \
                 baseline's identity bytes; regenerate it with the generate_irp_baselines example"
            );
        }
    }
    assert!(
        failures.is_empty(),
        "namespace runs diverge from the checked-in baselines: {failures:?}"
    );
}

#[test]
fn official_manual_cases_are_decided_only_by_their_manual_equivalent_oracle() {
    let root = irp_root();
    let manual_ids = ManualOracle::ALL.map(|oracle| oracle.case_id().to_owned());
    for datamodel in selected_data_models() {
        let options = RunOptions {
            filters: Filters {
                ids: manual_ids.to_vec(),
                ..Filters::default()
            },
            ..RunOptions::default()
        };
        let report = support::run_corpus(&root.join("prepared").join(datamodel), options);
        assert_eq!(report.manual_oracle.name, MANUAL_ORACLE_NAME);
        assert_eq!(report.manual_oracle.version, MANUAL_ORACLE_VERSION);
        assert_eq!(
            report
                .cases
                .iter()
                .map(|case| case.id.as_str())
                .collect::<Vec<_>>(),
            manual_ids.iter().map(String::as_str).collect::<Vec<_>>(),
            "every documented manual case is selected in the {datamodel} suite"
        );
        for case in &report.cases {
            // The W3C declares scripting-only 301 not applicable to XPath
            // (tests/conformance/w3c-irp/errata/xpath.json); it never runs
            // there and is reported, not hidden.
            if datamodel == "xpath" && case.id == "301" {
                assert_eq!(
                    (case.disposition, case.stage, case.oracle),
                    (Disposition::NotApplicable, Stage::Selection, None)
                );
                continue;
            }
            assert!(case.manual, "{datamodel} {} is a W3C manual case", case.id);
            // The unsatisfiable Outcome-oracle gates never decide a documented
            // manual case; its manual-equivalent oracle does.
            assert!(
                !matches!(
                    case.reason_code.as_deref(),
                    Some("outcome-pass-unreachable" | "outcome-log-missing-source")
                ),
                "{datamodel} {} must not be gated by the Outcome oracle",
                case.id
            );
            let decided = matches!(
                (case.disposition, case.stage),
                (Disposition::Passed, Stage::Complete) | (Disposition::Failed, Stage::Oracle)
            );
            assert_eq!(
                case.oracle,
                decided.then_some(OracleKind::ManualEquivalent),
                "{datamodel} {} oracle kind",
                case.id
            );
        }
        let case_415 = report
            .cases
            .iter()
            .find(|case| case.id == "415")
            .expect("official case 415 must remain in the prepared corpus");
        if datamodel == "ecmascript" {
            assert_eq!(case_415.disposition, Disposition::Passed);
            assert_eq!(case_415.oracle, Some(OracleKind::ManualEquivalent));
            assert_eq!(case_415.configuration, ["final"]);
        }
        report
            .canonical_artifact()
            .expect("manual-equivalent passes must validate as a canonical artifact");
    }
}

/// The audit's negative control, run through the shipped components: official
/// case 355 edited so its first transition logs `Outcome` `pass` and enters
/// the non-final state `s1`, where the session stays running. The W3C
/// success criterion is reaching the `conf:pass` final state
/// (`templates/ecmascript.xsl:18-30`), so the run is not `Passed`.
#[test]
fn an_official_case_that_logs_pass_and_keeps_running_is_not_passed() {
    const EDITED_355: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" datamodel="ecmascript" version="1.0">
<state id="s0">
  <transition target="s1"><log label="Outcome" expr="'pass'"/></transition>
</state>
<state id="s1"/>
<final id="pass"><onentry><log label="Outcome" expr="'pass'"/></onentry></final>
<final id="fail"><onentry><log label="Outcome" expr="'fail'"/></onentry></final>
</scxml>"#;
    let mut suite = scxml_conformance::load_suite(&irp_root().join("prepared/ecmascript"))
        .expect("the prepared corpus loads");
    suite.cases.retain(|case| case.manifest.id == "355");
    suite.cases[0].source = Some(EDITED_355.to_owned());
    let report = support::run(suite, RunOptions::default());
    let case = &report.cases[0];
    assert_eq!(case.id, "355");
    assert_eq!(
        (case.disposition, case.stage, case.reason_code.as_deref()),
        (
            Disposition::Failed,
            Stage::Oracle,
            Some("outcome-pass-not-final")
        ),
        "{case:?}"
    );
    assert_eq!(
        case.terminal_status,
        Some(scxml_conformance::TerminalStatus::Running)
    );
    assert_eq!(case.configuration, ["s1"]);
}

fn assert_measured_baseline(datamodel: &str, report: &RunReport) {
    // Every selected IRP case runs always: manual cases execute through the
    // same pipeline and the oracle decides their disposition, so nothing is
    // skipped. Only the W3C-declared not-applicable XPath cases leave the
    // denominator (tests/conformance/w3c-irp/errata/xpath.json: 301-304 are
    // scripting-only, 436 fixes datamodel="null", and the 19
    // ECMAScript-profile cases fix datamodel="ecmascript"), and every
    // applicable case passes.
    const XPATH_NOT_APPLICABLE: &[&str] = &[
        "301", "302", "303", "304", "436", "444", "445", "446", "448", "449", "451", "452", "453",
        "456", "457", "459", "460", "557", "558", "560", "561", "562", "569", "578",
    ];
    const XPATH_ERRATA: &[&str] = &["241", "244", "307", "520", "534"];
    // ECMAScript: 436 fixes datamodel="null"
    // (tests/conformance/w3c-irp/errata/ecmascript.json).
    const ECMASCRIPT_NOT_APPLICABLE: &[&str] = &["436"];
    let (expected_not_applicable, expected_errata): (&[&str], &[&str]) =
        if datamodel == "ecmascript" {
            (ECMASCRIPT_NOT_APPLICABLE, &[])
        } else {
            (XPATH_NOT_APPLICABLE, XPATH_ERRATA)
        };
    let applicable = 202 - expected_not_applicable.len() as u64;
    assert_eq!(report.summary.total_selected, 202);
    assert_eq!(report.summary.applicable_selected, applicable);
    assert_eq!(report.summary.passed, applicable);
    assert_eq!(report.summary.failed, 0);
    assert_eq!(report.summary.unsupported, 0);
    assert_eq!(report.summary.invalid, 0);
    assert_eq!(report.summary.harness_error, 0);
    assert_eq!(report.summary.skipped, 0);
    assert_eq!(report.summary.not_passing, 0);
    assert_eq!(
        report.summary.not_applicable,
        expected_not_applicable.len() as u64
    );
    assert_eq!(
        report.summary.passed_under_erratum,
        expected_errata.len() as u64
    );

    let ids_with = |disposition: Disposition| {
        report
            .cases
            .iter()
            .filter(|case| case.disposition == disposition)
            .map(|case| case.id.as_str())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        ids_with(Disposition::NotApplicable),
        expected_not_applicable.iter().copied().collect()
    );
    let all_ids = report
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids_with(Disposition::Passed),
        all_ids
            .difference(&expected_not_applicable.iter().copied().collect())
            .copied()
            .collect()
    );
    let erratum_ids = report
        .cases
        .iter()
        .filter(|case| case.disposition == Disposition::Passed && case.erratum.is_some())
        .map(|case| case.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(erratum_ids, expected_errata.iter().copied().collect());

    for manual in ManualOracle::ALL {
        let case = report
            .cases
            .iter()
            .find(|case| case.id == manual.case_id())
            .expect("every documented manual case must remain in the prepared corpus");
        if expected_not_applicable.contains(&case.id.as_str()) {
            assert_eq!(case.oracle, None, "{datamodel} {} never runs", case.id);
            continue;
        }
        assert_eq!(
            case.disposition,
            Disposition::Passed,
            "{datamodel} {}",
            case.id
        );
        assert_eq!(case.oracle, Some(OracleKind::ManualEquivalent));
    }

    let case_355 = report
        .cases
        .iter()
        .find(|case| case.id == "355")
        .expect("official case 355 must remain in the prepared corpus");
    assert_eq!(case_355.disposition, Disposition::Passed);
    assert_eq!(case_355.stage, Stage::Complete);
    assert_eq!(case_355.configuration, ["pass"]);
    assert_eq!(case_355.commands, 2);
}

/// A delayed `BasicHTTP` send is dispatched only when its delay elapses
/// (SCXML 6.2: "It MUST dispatch the message only when the delay interval
/// elapses."): the conformance executor has no outbound transport, so the
/// POST outside the run's loopback address space cannot be dispatched and
/// the sender raises `error.communication` carrying the send's id (SCXML
/// 6.2.4, C.2) at the send's deadline, after the undelayed `tick` sent
/// with it.
#[test]
fn a_delayed_basichttp_send_is_dispatched_when_due_through_the_components() {
    use scxml_compiler_core::{CompileInput, Compiler, MachineIr, ScriptResolution};
    use scxml_conformance::CaseExecutor as _;
    use scxml_validator_core::{FileSchemaResolver, SchemaMode, Validator};
    use scxml_validator_xsd::XsdSchemaBackend;

    let source = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <onentry>
      <send id="post" type="http://www.w3.org/TR/scxml/#BasicHTTPEventProcessor"
            target="http://127.0.0.1:9/receiver" event="later" delay="1s"/>
      <send event="tick" delay="10ms"/>
    </onentry>
    <transition event="tick" target="t"/>
  </state>
  <state id="t">
    <transition event="error.communication" cond="_event.sendid === 'post'" target="done"/>
  </state>
  <final id="done"/>
</scxml>"#;
    let compiler = Compiler::new(
        Validator::new(FileSchemaResolver::new(), XsdSchemaBackend)
            .with_schema_mode(SchemaMode::Off),
    );
    let lowered = compiler.compile(CompileInput {
        source_name: "delayed-basichttp.scx".into(),
        source: source.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: Vec::new(),
        script_resolution: ScriptResolution::Runtime,
        event_schemas: std::collections::BTreeMap::new(),
    });
    let artifact = lowered
        .artifact
        .expect("the delayed BasicHTTP document lowers");
    let ir = MachineIr::from_json(&artifact.bytes).expect("the lowered IR decodes");
    let bytes = artifact.bytes;
    let result = support::with_executor(move |executor| {
        executor
            .execute(&bytes, &ir, &[], 1_000, &BTreeMap::new())
            .expect("a delayed BasicHTTP send runs")
    });
    assert_eq!(result.configuration, ["done"], "{result:?}");
}
