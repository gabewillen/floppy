//! Adversarial fixture coverage for canonical bytes and both reporters.

use std::collections::BTreeMap;

use scxml_reporting::{RustcRenderer, decode_canonical, encode_canonical, render_sarif};
use scxml_syntax::SourceId;
use scxml_validator_core::{MemorySchemaResolver, SchemaMode, Validator};
use scxml_validator_xsd::XsdSchemaBackend;

/// The source id of the documents these tests parse.
const SOURCE_ID: SourceId = SourceId::new(1);

const SOURCE: &str = include_str!("../../../tests/fixtures/validator-adversarial.scxml");

#[test]
fn broken_scxml_produces_ai_actionable_deterministic_artifacts() {
    let validator = Validator::new(MemorySchemaResolver::new(), XsdSchemaBackend)
        .with_schema_mode(SchemaMode::Off);
    let outcome = validator.validate_source(SOURCE_ID, "validator<&>-é.scxml", SOURCE);
    assert!(outcome.report.has_errors());

    let artifact = outcome
        .report
        .canonical_artifact(&outcome.source)
        .expect("adversarial report must produce a canonical artifact");
    let first = encode_canonical(&artifact).expect("adversarial artifact must encode");
    let second = encode_canonical(&artifact).expect("adversarial artifact must re-encode");
    assert_eq!(first, second);
    assert_eq!(
        decode_canonical(&first).expect("adversarial canonical bytes must decode"),
        artifact
    );

    let payload = artifact.validator_payload().expect("validator payload");
    let by_code: BTreeMap<_, _> = payload
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.code.as_str(), diagnostic))
        .collect();
    for code in [
        "SCXML003", "SCXML006", "SCXML010", "SCXML011", "SCXML013", "SCXML014", "SCXML026",
    ] {
        assert!(
            by_code.contains_key(code),
            "missing adversarial rule {code}"
        );
    }
    assert!(payload.diagnostics.iter().all(|diagnostic| {
        diagnostic.help.is_some()
            && !diagnostic.spec_references.is_empty()
            && diagnostic.primary.span.end <= SOURCE.len() as u64
    }));
    assert!(
        payload
            .diagnostics
            .iter()
            .any(|diagnostic| !diagnostic.secondary.is_empty())
    );
    assert!(
        payload
            .diagnostics
            .iter()
            .any(|diagnostic| !diagnostic.fixes.is_empty())
    );

    let text = RustcRenderer::default()
        .render(&artifact)
        .expect("adversarial artifact must render as rustc text");
    assert!(text.contains("validator<&>-é.scxml:2:14"));
    assert!(text.contains("= help:"));
    assert!(text.contains("= spec:"));
    assert!(
        text.contains("\nhelp: "),
        "rustc-style inline suggestion expected:\n{text}"
    );

    let sarif = render_sarif(&artifact).expect("adversarial artifact must render as SARIF");
    let sarif: serde_json::Value =
        serde_json::from_slice(&sarif).expect("adversarial SARIF must be valid JSON");
    assert_eq!(sarif["version"], "2.1.0");
    assert_eq!(
        sarif["runs"][0]["artifacts"][0]["location"]["uri"],
        "validator%3C%26%3E-%C3%A9.scxml"
    );
    assert!(
        sarif["runs"][0]["results"]
            .as_array()
            .expect("SARIF results inventory must be an array")
            .iter()
            .any(|result| result["message"]["markdown"]
                .as_str()
                .is_some_and(|markdown| markdown.contains("**Help:**")))
    );
}
