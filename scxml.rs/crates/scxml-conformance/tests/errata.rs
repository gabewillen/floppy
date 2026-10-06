#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

//! Checked-in IRP errata: preparation applies corrections deterministically
//! and fails closed on drift or unknown cases; runs report erratum passes and
//! W3C not-applicable cases distinctly and outside the denominator.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "wasmtime")]
mod support;

use scxml_conformance::{
    CorpusTransformer, ErratumStage, NotApplicable, PrepareError, PrepareOptions, SuiteManifest,
    TransformerIdentity, prepare_corpus, sha256_bytes, suite_identity_digest,
};
#[cfg(feature = "wasmtime")]
use scxml_conformance::{Disposition, RunOptions, RunReport};

const TEMPLATE: &str = "<stylesheet/>";
const CORRECTED_TEMPLATE: &str = "<stylesheet corrected=\"1\"/>";

/// Deterministic stand-in for the XSLT transformer: the output is the source
/// followed by the template as an XML comment, so both erratum stages change
/// the output bytes.
struct ConcatTransformer;

impl CorpusTransformer for ConcatTransformer {
    fn identity(&self) -> Result<TransformerIdentity, PrepareError> {
        Ok(TransformerIdentity {
            name: "concat-xslt".into(),
            version: "1".into(),
            command_argv: vec!["test-fixture".into()],
            executable_digest: None,
        })
    }

    fn transform(&self, source: &Path, template: &Path, output: &Path) -> Result<(), PrepareError> {
        let source = fs::read_to_string(source).expect("fixture source must be readable");
        let template = fs::read_to_string(template).expect("fixture template must be readable");
        fs::write(output, transformed(&source, &template)).expect("fixture output must write");
        Ok(())
    }
}

fn transformed(source: &str, template: &str) -> String {
    format!("{source}\n<!--{template}-->")
}

fn machine(first_target: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><transition target="{first_target}"/></state><final id="pass"><onentry><log label="Outcome" expr="'pass'"/></onentry></final><final id="fail"><onentry><log label="Outcome" expr="'fail'"/></onentry></final></scxml>"#
    )
}

struct Fixture {
    root: PathBuf,
    options: PrepareOptions,
    errata: PathBuf,
}

/// Case 101 is broken in its source (source erratum), 102 carries a template
/// erratum, 103 is declared not applicable, and 104 is untouched.
fn fixture(name: &str) -> Fixture {
    let temporary = fs::canonicalize(std::env::temp_dir())
        .expect("system temporary directory must have a canonical path");
    let root = temporary.join(format!("scxml-errata-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let source = root.join("source");
    let templates = root.join("templates");
    fs::create_dir_all(&templates).expect("template directory must be created");
    let mut manifest = String::from(r#"<assertions baseuri="https://example.invalid/irp">"#);
    for (id, body) in [
        ("101", machine("fail")),
        ("102", machine("pass")),
        ("103", machine("fail")),
        ("104", machine("pass")),
    ] {
        fs::create_dir_all(source.join(id)).expect("case directory must be created");
        fs::write(source.join(id).join(format!("test{id}.txml")), body)
            .expect("case source must be writable");
        write!(
            manifest,
            r##"<assert id="{id}" specnum="3.2" specid="#scxml"><test id="{id}" conformance="mandatory" manual="false"><start uri="{id}/test{id}.txml"/></test></assert>"##
        )
        .expect("writing to a String cannot fail");
    }
    manifest.push_str("</assertions>");
    fs::write(source.join("manifest.xml"), manifest).expect("manifest must be writable");
    fs::write(templates.join("ecmascript.xsl"), TEMPLATE).expect("template must be writable");
    let errata = root.join("ecmascript.json");
    let options = PrepareOptions {
        manifest: source.join("manifest.xml").display().to_string(),
        base_uri: None,
        templates,
        datamodel: "ecmascript".into(),
        cache: root.join("cache"),
        output: root.join("output"),
        source_dir: Some(source),
        revision: Some("fixture-revision".into()),
        offline: false,
        force: false,
        max_commands: 64,
        errata: Some(errata.clone()),
        namespace: None,
    };
    let fixture = Fixture {
        root,
        options,
        errata,
    };
    write_errata(&fixture, &errata_value());
    fixture
}

fn errata_value() -> serde_json::Value {
    serde_json::json!({
        "errata_version": 1,
        "datamodel": "ecmascript",
        "template_sha256": sha256_bytes(TEMPLATE.as_bytes()),
        "errata": [
            {
                "id": "fixture-101-target",
                "case": "101",
                "stage": "source",
                "defect": "the source targets fail",
                "evidence": ["101/test101.txml:1"],
                "correction": "target pass",
                "replacements": [{"find": "<transition target=\"fail\"/>", "replace": "<transition target=\"pass\"/>"}],
                "upstream_output_sha256": sha256_bytes(transformed(&machine("fail"), TEMPLATE).as_bytes()),
                "corrected_output_sha256": sha256_bytes(transformed(&machine("pass"), TEMPLATE).as_bytes())
            },
            {
                "id": "fixture-102-template",
                "case": "102",
                "stage": "template",
                "defect": "the template is uncorrected",
                "evidence": ["ecmascript.xsl:1"],
                "correction": "mark the template corrected",
                "replacements": [{"find": TEMPLATE, "replace": CORRECTED_TEMPLATE}],
                "upstream_output_sha256": sha256_bytes(transformed(&machine("pass"), TEMPLATE).as_bytes()),
                "corrected_output_sha256": sha256_bytes(transformed(&machine("pass"), CORRECTED_TEMPLATE).as_bytes())
            }
        ],
        "not_applicable": [
            {"case": "103", "reason": "Not applicable per W3C.", "evidence": ["manifest.xml:1"]}
        ]
    })
}

fn write_errata(fixture: &Fixture, value: &serde_json::Value) {
    fs::write(
        &fixture.errata,
        serde_json::to_vec_pretty(value).expect("errata fixture must encode"),
    )
    .expect("errata fixture must be writable");
}

fn prepare_error(fixture: &Fixture) -> PrepareError {
    prepare_corpus(&fixture.options, &ConcatTransformer)
        .expect_err("defective errata must fail preparation closed")
}

#[test]
fn errata_apply_deterministically_and_record_their_identity() {
    let fixture = fixture("apply");
    let summary =
        prepare_corpus(&fixture.options, &ConcatTransformer).expect("valid errata must prepare");
    let corpus_bytes = fs::read(&summary.corpus).expect("corpus must be readable");
    let suite = scxml_conformance::load_suite(&summary.corpus)
        .expect("errata corpus must pass loader verification");
    let identity = suite
        .manifest
        .suite
        .errata
        .as_ref()
        .expect("prepared suite must record the errata identity");
    assert_eq!(identity.source, "ecmascript.json");
    assert_eq!(
        identity.digest,
        sha256_bytes(&fs::read(&fixture.errata).expect("errata must be readable"))
    );
    assert_eq!(identity.applied.len(), 2);
    assert_eq!(identity.applied["101"], "fixture-101-target");
    assert_eq!(identity.not_applicable, ["103"]);
    let case = |id: &str| {
        suite
            .cases
            .iter()
            .find(|case| case.manifest.id == id)
            .expect("fixture case must be prepared")
    };
    let corrected = case("101");
    let erratum = corrected
        .manifest
        .erratum
        .as_ref()
        .expect("case 101 must record its erratum");
    assert_eq!(erratum.stage, ErratumStage::Source);
    assert_eq!(
        erratum.upstream_output_digest,
        sha256_bytes(transformed(&machine("fail"), TEMPLATE).as_bytes())
    );
    assert_eq!(
        corrected.source.as_deref(),
        Some(transformed(&machine("pass"), TEMPLATE).as_str()),
        "the executed source must be the corrected output"
    );
    assert_eq!(
        case("102")
            .manifest
            .erratum
            .as_ref()
            .expect("case 102 must record its erratum")
            .stage,
        ErratumStage::Template
    );
    assert!(case("103").manifest.not_applicable.is_some());
    assert!(case("104").manifest.erratum.is_none());

    // Regenerating every artifact from scratch yields identical bytes.
    let mut forced = fixture.options.clone();
    forced.force = true;
    let again = prepare_corpus(&forced, &ConcatTransformer).expect("forced errata must prepare");
    assert_eq!(
        fs::read(&again.corpus).expect("regenerated corpus must be readable"),
        corpus_bytes,
        "errata application must be byte-deterministic"
    );

    fs::remove_dir_all(&fixture.root).expect("fixture directory must be removable");
}

#[cfg(feature = "wasmtime")]
#[test]
fn erratum_passes_and_not_applicable_cases_are_reported_distinctly_through_components() {
    let fixture = fixture("run");
    let summary =
        prepare_corpus(&fixture.options, &ConcatTransformer).expect("valid errata must prepare");
    let corpus_bytes = fs::read(&summary.corpus).expect("corpus must be readable");
    let report = support::run_corpus(&summary.corpus, RunOptions::default());
    let disposition = |id: &str| {
        report
            .cases
            .iter()
            .find(|case| case.id == id)
            .map(|case| (case.disposition, case.erratum.clone()))
            .expect("fixture case must be reported")
    };
    assert_eq!(
        disposition("101"),
        (Disposition::Passed, Some("fixture-101-target".into()))
    );
    assert_eq!(
        disposition("102"),
        (Disposition::Passed, Some("fixture-102-template".into()))
    );
    assert_eq!(disposition("103"), (Disposition::NotApplicable, None));
    assert_eq!(disposition("104"), (Disposition::Passed, None));
    assert_eq!(report.summary.total_selected, 4);
    assert_eq!(report.summary.applicable_selected, 3);
    assert_eq!(report.summary.passed, 3);
    assert_eq!(report.summary.passed_under_erratum, 2);
    assert_eq!(report.summary.not_applicable, 1);
    assert_eq!(report.summary.not_passing, 0);
    assert_eq!(report.summary.mandatory.applicable_selected, 3);
    assert_eq!(report.summary.mandatory.not_applicable, 1);
    assert!(report.summary.mandatory_complete());
    assert_tampering_fails_closed(&fixture, &report, &summary.corpus, &corpus_bytes);
    fs::remove_dir_all(&fixture.root).expect("fixture directory must be removable");
}

/// The report round-trips canonically, while a report that invents an erratum
/// and a corpus case that names an erratum id absent from the suite identity
/// both fail closed.
#[cfg(feature = "wasmtime")]
fn assert_tampering_fails_closed(
    fixture: &Fixture,
    report: &RunReport,
    corpus: &Path,
    corpus_bytes: &[u8],
) {
    report
        .canonical_artifact()
        .expect("errata report must satisfy the canonical reporting contract");
    let path = fixture.root.join("report.json");
    report.write_json(&path).expect("errata report must write");
    assert_eq!(
        RunReport::read_json(&path).expect("errata report must read back"),
        *report
    );

    // A report that hides an erratum or invents one fails closed.
    let mut invented = report.clone();
    if let Some(case) = invented.cases.iter_mut().find(|case| case.id == "104") {
        case.erratum = Some("fixture-unknown".into());
    }
    let error = invented
        .write_json(&fixture.root.join("invented.json"))
        .expect_err("an unrecorded erratum id must be rejected");
    assert_eq!(error.code, "report-case-errata");

    // A prepared case naming an unknown erratum id fails loading.
    let mut value: serde_json::Value =
        serde_json::from_slice(corpus_bytes).expect("corpus must be JSON");
    for case in value["cases"]
        .as_array_mut()
        .expect("corpus cases must be an array")
    {
        if case["id"] == "101" {
            case["erratum"]["id"] = serde_json::json!("fixture-unknown");
        }
    }
    fs::write(
        corpus,
        serde_json::to_vec_pretty(&value).expect("tampered corpus must encode"),
    )
    .expect("tampered corpus must be writable");
    let error = scxml_conformance::load_suite(corpus)
        .expect_err("an erratum id absent from the suite identity must be rejected");
    assert_eq!(error.code, "case-erratum");
}

/// Rewrites the prepared corpus manifest with `change` applied and its
/// corpus identity recomputed, as a crafted corpus would.
fn rewrite_corpus_with_identity(corpus: &Path, change: impl FnOnce(&mut SuiteManifest)) {
    let mut manifest: SuiteManifest =
        serde_json::from_slice(&fs::read(corpus).expect("corpus must be readable"))
            .expect("corpus must decode");
    change(&mut manifest);
    manifest.suite.corpus_digest =
        suite_identity_digest(&manifest).expect("the crafted corpus identity must recompute");
    let mut bytes = serde_json::to_vec_pretty(&manifest).expect("corpus must encode");
    bytes.push(b'\n');
    fs::write(corpus, bytes).expect("crafted corpus must be writable");
}

fn load_error(corpus: &Path) -> String {
    scxml_conformance::load_suite(corpus)
        .expect_err(
            "a corpus whose errata declarations are not the bound errata bytes must be rejected",
        )
        .code
        .to_owned()
}

#[test]
fn a_prepared_corpus_carries_its_errata_bytes_and_the_loader_rehashes_them() {
    let fixture = fixture("bound");
    let summary =
        prepare_corpus(&fixture.options, &ConcatTransformer).expect("valid errata must prepare");
    let corpus = summary.corpus;
    let pristine = fs::read(&corpus).expect("corpus must be readable");
    let bound = fixture.options.output.join("ecmascript.json");
    let errata_bytes = fs::read(&fixture.errata).expect("errata must be readable");
    assert_eq!(
        fs::read(&bound).expect("preparation must write the errata bytes into the corpus"),
        errata_bytes,
        "the corpus carries the exact errata bytes it binds"
    );
    scxml_conformance::load_suite(&corpus).expect("the prepared corpus must load");

    // A tampered errata file fails the declared digest.
    let tampered = String::from_utf8(errata_bytes.clone())
        .expect("errata fixture is UTF-8")
        .replace("Not applicable per W3C.", "Not applicable, trust me.");
    fs::write(&bound, tampered).expect("tampered errata must be writable");
    assert_eq!(load_error(&corpus), "suite-errata-digest");
    fs::remove_file(&bound).expect("bound errata must be removable");
    assert_eq!(load_error(&corpus), "suite-errata-read");
    fs::write(&bound, &errata_bytes).expect("errata must be restorable");

    // A crafted corpus declares a case not applicable that the errata do
    // not, with a recomputed corpus identity.
    rewrite_corpus_with_identity(&corpus, |manifest| {
        if let Some(errata) = manifest.suite.errata.as_mut() {
            errata.not_applicable.push("104".into());
        }
        for case in &mut manifest.cases {
            if case.id == "104" {
                case.not_applicable = Some(NotApplicable {
                    reason: "Not applicable per W3C. Evidence: manifest.xml:1".into(),
                });
            }
        }
    });
    assert_eq!(load_error(&corpus), "suite-errata-identity");
    fs::write(&corpus, &pristine).expect("corpus must be restorable");

    // A crafted corpus rewrites a declared case's reason.
    rewrite_corpus_with_identity(&corpus, |manifest| {
        for case in &mut manifest.cases {
            if case.id == "103" {
                case.not_applicable = Some(NotApplicable {
                    reason: "Not applicable, trust me.".into(),
                });
            }
        }
    });
    assert_eq!(load_error(&corpus), "suite-errata-case");
    fs::write(&corpus, &pristine).expect("corpus must be restorable");

    // A crafted corpus names an errata file outside the corpus directory.
    rewrite_corpus_with_identity(&corpus, |manifest| {
        if let Some(errata) = manifest.suite.errata.as_mut() {
            errata.source = "../ecmascript.json".into();
        }
    });
    assert_eq!(load_error(&corpus), "suite-errata-source");
    fs::write(&corpus, &pristine).expect("corpus must be restorable");

    // A crafted corpus records a corrected case's erratum at another stage
    // than the bound errata declare.
    rewrite_corpus_with_identity(&corpus, |manifest| {
        for case in &mut manifest.cases {
            if case.id == "102"
                && let Some(erratum) = case.erratum.as_mut()
            {
                erratum.stage = ErratumStage::Source;
            }
        }
    });
    assert_eq!(load_error(&corpus), "suite-errata-case");

    fs::remove_dir_all(&fixture.root).expect("fixture directory must be removable");
}

#[test]
fn errata_digest_drift_fails_closed() {
    let fixture = fixture("drift");
    let mut value = errata_value();
    value["errata"][0]["upstream_output_sha256"] = serde_json::json!("0".repeat(64));
    write_errata(&fixture, &value);
    assert_eq!(prepare_error(&fixture).code, "erratum-upstream-drift");
    assert!(
        !fixture.options.output.join("corpus.json").exists(),
        "a drifted erratum must not publish a corpus"
    );

    let mut value = errata_value();
    value["errata"][1]["corrected_output_sha256"] = serde_json::json!("0".repeat(64));
    write_errata(&fixture, &value);
    assert_eq!(prepare_error(&fixture).code, "erratum-corrected-drift");

    let mut value = errata_value();
    value["template_sha256"] = serde_json::json!("0".repeat(64));
    write_errata(&fixture, &value);
    assert_eq!(prepare_error(&fixture).code, "errata-template-drift");

    let mut value = errata_value();
    value["errata"][0]["replacements"][0]["find"] = serde_json::json!("<absent/>");
    write_errata(&fixture, &value);
    assert_eq!(prepare_error(&fixture).code, "erratum-patch-mismatch");
    fs::remove_dir_all(&fixture.root).expect("fixture directory must be removable");
}

#[test]
fn errata_naming_unknown_cases_or_other_data_models_fail_closed() {
    let fixture = fixture("unknown");
    let mut value = errata_value();
    value["errata"][0]["case"] = serde_json::json!("999");
    write_errata(&fixture, &value);
    assert_eq!(prepare_error(&fixture).code, "errata-unknown-case");

    let mut value = errata_value();
    value["not_applicable"][0]["case"] = serde_json::json!("998");
    write_errata(&fixture, &value);
    assert_eq!(prepare_error(&fixture).code, "errata-unknown-case");

    let mut value = errata_value();
    value["datamodel"] = serde_json::json!("xpath");
    write_errata(&fixture, &value);
    assert_eq!(prepare_error(&fixture).code, "errata-datamodel-mismatch");

    let mut value = errata_value();
    value["errata"][1]["id"] = serde_json::json!("fixture-101-target");
    write_errata(&fixture, &value);
    assert_eq!(prepare_error(&fixture).code, "errata-duplicate-id");

    let mut value = errata_value();
    value["unexpected"] = serde_json::json!(true);
    write_errata(&fixture, &value);
    assert_eq!(prepare_error(&fixture).code, "errata-json");
    fs::remove_dir_all(&fixture.root).expect("fixture directory must be removable");
}
