//! The `generate` service's package validator
//! (`services/generate/validator`) through `scx validate`: the dev package
//! cache `scripts/build-components.sh` publishes pins it with the generate
//! package, so a document invoking the service by its repository URI is
//! checked by it, and its findings are reported as `generate::<code>`. A
//! machine-applicable fix is applied with the `scx` namespace's fix pass
//! (`scx::apply_machine_applicable`, what `<scx:validate applyfixes>` runs)
//! to the canonical report, and the fixed document validates clean.

#![allow(
    missing_docs,
    reason = "integration-test crate; test functions are self-describing"
)]

mod support;

use support::{output, service_uri, unique_scratch, write};

fn document(params: &str, transitions: &str) -> String {
    format!(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" xmlns:event="https://github.com/stateforward/scxml.rs/namespaces/event" version="1.0" datamodel="ecmascript" initial="s">
  <state id="s">
    <invoke type="{}">
      <param name="model" expr="'m'"/>
      <param name="prompt" expr="'p'"/>
      {params}
    </invoke>
    {transitions}
    <transition event="done.invoke" target="f"/>
  </state>
  <final id="f"/>
</scxml>
"#,
        service_uri("generate")
    )
}

/// The canonical report of `scx validate --format json` of `text`.
fn validate(text: &str) -> (bool, scxml_reporting::ArtifactEnvelope) {
    let path = write(&unique_scratch("generate-validator"), "doc.scx", text);
    let validated = output(&[
        "validate".to_owned(),
        path.display().to_string(),
        "--format".to_owned(),
        "json".to_owned(),
    ]);
    let report = scxml_reporting::decode_canonical(validated.stdout.trim_end().as_bytes())
        .unwrap_or_else(|error| panic!("a canonical report: {error}: {}", validated.stderr));
    (validated.status.success(), report)
}

fn codes(report: &scxml_reporting::ArtifactEnvelope) -> Vec<String> {
    report
        .validator_payload()
        .expect("a validator report")
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code.clone())
        .collect()
}

#[test]
fn a_reasoning_effort_typo_is_fixed_by_the_fix_pass() {
    let text = document(r#"<param name="reasoning_effort" expr="'hgh'"/>"#, "");
    let (clean, report) = validate(&text);
    assert!(!clean);
    assert_eq!(codes(&report), ["generate::invalid_value"]);
    let fixed = scx::apply_machine_applicable(&report).expect("the fixes apply");
    assert!(fixed.contains(r#"expr="'high'""#), "{fixed}");
    let (clean, report) = validate(&fixed);
    assert!(clean && codes(&report).is_empty(), "{:?}", codes(&report));
}

#[test]
fn a_parameter_typo_is_renamed_by_the_fix_pass() {
    let text = document(r#"<param name="reasoning_efort" expr="'low'"/>"#, "");
    let (_, report) = validate(&text);
    assert_eq!(codes(&report), ["generate::unknown_param"]);
    let fixed = scx::apply_machine_applicable(&report).expect("the fixes apply");
    assert!(fixed.contains(r#"name="reasoning_effort""#), "{fixed}");
    assert!(codes(&validate(&fixed).1).is_empty());
}

#[test]
fn a_transition_schema_that_rejects_what_generate_returns_is_a_schema_mismatch() {
    let narrow = r#"<transition event="generate.failed" event:schema='{"type":"object","properties":{"code":{"enum":["transport"]}},"required":["code"]}'/>"#;
    let (clean, report) = validate(&document("", narrow));
    // A warning: the document still validates.
    assert!(clean);
    assert_eq!(codes(&report), ["generate::schema_mismatch"]);
    let help = report
        .validator_payload()
        .expect("a validator report")
        .diagnostics[0]
        .help
        .clone()
        .unwrap_or_default();
    assert!(help.contains("\"schema-mismatch\""), "{help}");
}
