//! The validator's null data-model condition grammar equals the shipped null
//! data-model component's: a `cond` gets no `SCXML037` finding exactly when
//! `NullDataModel::evaluate_condition` accepts it. A document the validator
//! calls clean must never fail its conditions at run time, and the validator
//! must not reject a condition the component evaluates.

use scxml_data_model_null::{ExecutionHost, NullDataModel, NullDataModelError};
use scxml_syntax::SourceId;
use scxml_validator_core::{MemorySchemaResolver, Validator};
use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

/// The source id of every validated document.
const SOURCE: SourceId = SourceId::new(1);

/// Host that reports every state active; only acceptance is compared.
struct AllActive;

impl ExecutionHost for AllActive {
    fn in_state(&mut self, _state_id: &str) -> Result<bool, NullDataModelError> {
        Ok(true)
    }
}

/// Conditions an author (or an agent) writes under `datamodel="null"`.
const CONDITIONS: &[&str] = &[
    "In('s')",
    "In(\"s\")",
    " In ( 's' ) ",
    "In(s)",
    "In( s )",
    "In(f)",
    "In('')",
    "In(\"\")",
    "In('a b')",
    "In('s\\')",
    "In('a\"b')",
    "In(\"a'b\")",
    "In('s)')",
    "In('s'",
    "In 's'",
    "in('s')",
    "!In('s')",
    "In('s') && In('f')",
    "In('s', 'f')",
    "true",
    "x > 1",
    "In()",
    "In(s-1)",
    "In(s.1)",
    "In(1s)",
    "In('a\u{1680}b')",
    "In('a\u{a0}b')",
    "\u{1680}In('s')",
    "In(\u{a0}'s')",
    "In('s')\u{2028}",
];

/// Escapes a condition for a double-quoted XML attribute.
fn attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

#[test]
fn validator_accepts_exactly_the_conditions_the_null_component_evaluates() {
    let validator = Validator::new(
        BundledSchemaResolver::new(MemorySchemaResolver::new()),
        XsdSchemaBackend,
    );
    let mut disagreements = Vec::new();
    for condition in CONDITIONS {
        let source = format!(
            "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"null\" initial=\"s\"><state id=\"s\"><transition event=\"go\" cond=\"{}\" target=\"f\"/></state><final id=\"f\"/></scxml>",
            attribute(condition)
        );
        let outcome = validator.validate_source(SOURCE, "null.scxml", source);
        let validator_reports = outcome
            .report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "SCXML079");
        let runtime_rejects = NullDataModel
            .evaluate_condition(condition, &mut AllActive)
            .is_err();
        if validator_reports != runtime_rejects {
            disagreements.push(format!(
                "{condition:?}: validator reports = {validator_reports}, null component rejects = {runtime_rejects}"
            ));
        }
    }
    assert!(
        disagreements.is_empty(),
        "validator and null data-model component disagree:\n{}",
        disagreements.join("\n")
    );
}

/// White space in XML, and so in the null data model's `In()` grammar, is
/// #x20, #x9, #xD and #xA only: U+1680 OGHAM SPACE MARK is an XML name
/// character, so `In('a&#x1680;b')` names the state `a&#x1680;b`.
#[test]
fn in_names_a_state_whose_id_holds_other_unicode_white_space() {
    let validator = Validator::new(
        BundledSchemaResolver::new(MemorySchemaResolver::new()),
        XsdSchemaBackend,
    );
    let source = "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"null\" initial=\"a&#x1680;b\"><state id=\"a&#x1680;b\"><transition event=\"go\" cond=\"In('a&#x1680;b')\" target=\"f\"/></state><final id=\"f\"/></scxml>";
    let outcome = validator.validate_source(SOURCE, "null.scxml", source);
    assert!(
        outcome.report.diagnostics().is_empty(),
        "{:?}",
        outcome.report.diagnostics()
    );
    assert_eq!(
        NullDataModel.evaluate_condition("In('a\u{1680}b')", &mut AllActive),
        Ok(true)
    );
}
