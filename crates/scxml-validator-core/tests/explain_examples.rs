//! Every self-checking `--explain` example emits its code and its corrected
//! example does not, validated with the bundled W3C schema and the ECMAScript
//! and `XPath` expression checkers, composed exactly as `scx validate`
//! composes them.

use scxml_validator_core::ExternalScripts;
use std::sync::Arc;

use scxml_data_model_ecmascript::{SyntaxKind, check_syntax};
use scxml_reporting::explain::{CODES, CodeDoc};
use scxml_syntax::SourceId;
mod support;

use scxml_validator_core::{
    EffectCheck, EffectFragment, ExpressionCheck, ExpressionKind, ExpressionSyntaxChecker,
    MemorySchemaResolver, SyntaxIssue, Validator,
};
use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

/// Parse-only ECMAScript checker, as the CLI links it. Unoptimized test
/// builds need a large native stack per Boa parser level, so each check runs
/// on its own thread.
#[derive(Debug)]
struct EcmaScriptChecker;

impl ExpressionSyntaxChecker for EcmaScriptChecker {
    fn data_model(&self) -> &'static str {
        "ecmascript"
    }

    fn check(&self, kind: ExpressionKind, source: &str) -> ExpressionCheck {
        let kind = match kind {
            ExpressionKind::Condition => SyntaxKind::Condition,
            ExpressionKind::Value => SyntaxKind::Value,
            ExpressionKind::Location => SyntaxKind::Location,
            ExpressionKind::Script => SyntaxKind::Script,
        };
        let source = source.to_owned();
        let result = std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || check_syntax(kind, &source))
            .expect("checker thread spawns")
            .join()
            .expect("bounded parse does not overflow a 64 MiB stack");
        match result {
            Ok(issues) if issues.is_empty() => ExpressionCheck::Valid,
            Ok(issues) => ExpressionCheck::Invalid(
                issues
                    .into_iter()
                    .map(|issue| SyntaxIssue {
                        start: issue.start,
                        end: issue.end,
                        message: issue.message,
                    })
                    .collect(),
            ),
            Err(_) => ExpressionCheck::NotChecked,
        }
    }

    fn check_effects(
        &self,
        fragments: &[EffectFragment<'_>],
        scripts: ExternalScripts,
    ) -> EffectCheck {
        support::ecmascript_effects(fragments, scripts)
    }
}

/// Parse-only `XPath` checker, as the CLI links it.
#[derive(Debug)]
struct XPathChecker;

impl ExpressionSyntaxChecker for XPathChecker {
    fn data_model(&self) -> &'static str {
        "xpath"
    }

    fn check(&self, kind: ExpressionKind, source: &str) -> ExpressionCheck {
        let kind = match kind {
            ExpressionKind::Condition => scxml_data_model_xpath::SyntaxKind::Condition,
            ExpressionKind::Value => scxml_data_model_xpath::SyntaxKind::Value,
            ExpressionKind::Location => scxml_data_model_xpath::SyntaxKind::Location,
            ExpressionKind::Script => scxml_data_model_xpath::SyntaxKind::Script,
        };
        match scxml_data_model_xpath::check_syntax(kind, source) {
            Ok(issues) if issues.is_empty() => ExpressionCheck::Valid,
            Ok(issues) => ExpressionCheck::Invalid(
                issues
                    .into_iter()
                    .map(|issue| SyntaxIssue {
                        start: issue.start,
                        end: issue.end,
                        message: issue.message,
                    })
                    .collect(),
            ),
            Err(_) => ExpressionCheck::NotChecked,
        }
    }

    fn check_effects(
        &self,
        _fragments: &[EffectFragment<'_>],
        _scripts: ExternalScripts,
    ) -> EffectCheck {
        EffectCheck::Checked(Vec::new())
    }
}

/// Entries whose examples cannot be validated as documents by
/// `scx validate`, with the reason. Every other entry is
/// self-checking; the list must not grow without a reason, and an entry in
/// it must not become self-checking unnoticed.
const NOT_SELF_CHECKING: &[(&str, &str)] = &[
    ("COMP001", "produced only by `compile` lowering"),
    ("COMP002", "produced only by `compile` package binding"),
    (
        "COMP003",
        "produced only by `compile` script-package pinning",
    ),
    (
        "PKG001",
        "produced only by the validator component's package requests",
    ),
    (
        "SCXML088",
        "needs a schema file next to the document; covered by the UI case scxml088-missing-schema-file",
    ),
    (
        "SCXML089",
        "needs a schema file to point into; covered by the UI case scxml089-bad-pointer",
    ),
    (
        "SCXML045",
        "a note only when no checker for the machine's data model is linked, and this harness links both",
    ),
    (
        "SCXML027",
        "needs 64 nested inline machines; the example is prose",
    ),
    ("SCXML028", "needs 65,536 states; the example is prose"),
    (
        "SCXML069",
        "needs a 65-deep hierarchy or 1 MiB values; the example is prose",
    ),
    (
        "SCXML062",
        "the example would print a raw bidirectional control to the terminal; it is prose",
    ),
    ("SCXML041", "needs 16,385 expressions; the example is prose"),
    ("XML002", "needs 257 nested elements; the example is prose"),
    (
        "XML005",
        "a caller configuration error, not a document finding",
    ),
    (
        "XSD001",
        "needs an extension namespace without a registered schema",
    ),
    (
        "SCXML081",
        "the example needs an ID of more than 1,013 bytes; tests/ui covers it",
    ),
    (
        "SCXML082",
        "the example needs a name of more than 1,012 bytes; tests/ui covers it",
    ),
    (
        "SCXML083",
        "the example needs an extension package's declared slots; the validator's env attribute tests cover it",
    ),
    (
        "SCXML084",
        "the example is a fragment; the validator's env attribute tests cover it",
    ),
    (
        "SCXML094",
        "the example needs the validator's bound service types; the validator's invoke type tests cover it",
    ),
    (
        "SCXML097",
        "the example needs the draft profile; the validator's draft profile tests cover it",
    ),
    (
        "SCXML098",
        "the example needs the draft profile; the validator's draft profile tests cover it",
    ),
    (
        "SCXML099",
        "the example needs the draft profile; the validator's draft profile tests cover it",
    ),
    (
        "SCXML100",
        "the example needs the draft profile; the validator's draft profile tests cover it",
    ),
    (
        "SCXML101",
        "the example needs the draft profile; the validator's draft profile tests cover it",
    ),
    ("XSD002", "the example is a schema, not an SCXML document"),
    ("XSD003", "the example is a schema, not an SCXML document"),
    ("XSD004", "the example is a schema, not an SCXML document"),
    (
        "XSD005",
        "no known document triggers the backend driver failure",
    ),
    ("XSD006", "the example is a schema, not an SCXML document"),
    ("XSD007", "the example is a schema, not an SCXML document"),
    (
        "XSD-src-resolve",
        "a family with no example; needs a broken custom schema",
    ),
    (
        "XSD-cvc-attribute",
        "a family documented by its sub-code entries",
    ),
    (
        "XSD-cvc-complex-type",
        "a family documented by its sub-code entries",
    ),
    (
        "XSD-cvc-datatype-valid",
        "a family documented by its sub-code entries",
    ),
    ("XSD-cvc-elt", "a family documented by its sub-code entries"),
    (
        "XSD-cvc-simple-type",
        "a family documented by its sub-code entries",
    ),
    (
        "XSD-cvc-type",
        "a family documented by its sub-code entries",
    ),
];

#[test]
fn every_entry_is_self_checking_or_has_a_documented_reason() {
    for entry in CODES {
        let exempt = NOT_SELF_CHECKING
            .iter()
            .any(|(code, _)| *code == entry.code);
        assert!(
            entry.self_checking != exempt,
            "{}: self-checking must be exactly the entries not listed in NOT_SELF_CHECKING",
            entry.code
        );
        // Prose placeholders are never presented as XML.
        for example in [entry.bad, entry.fixed] {
            if example.contains('…') {
                assert!(
                    !scxml_reporting::explain::explanation(entry)
                        .contains(&format!("```xml\n{example}\n```")),
                    "{}: a prose example is rendered as XML",
                    entry.code
                );
            }
        }
    }
}

#[test]
fn self_checking_examples_trigger_their_code_and_fixes_remove_it() {
    let validator = Validator::new(
        BundledSchemaResolver::new(MemorySchemaResolver::new()),
        XsdSchemaBackend,
    )
    .with_expression_checker(Arc::new(EcmaScriptChecker))
    .with_expression_checker(Arc::new(XPathChecker));
    let matches = |entry: &CodeDoc, code: &str| {
        if entry.family {
            code.strip_prefix(entry.code)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
        } else {
            code == entry.code
        }
    };
    let mut failures = Vec::new();
    for entry in CODES.iter().filter(|entry| entry.self_checking) {
        let bad = validator.validate_source(SOURCE, "bad.scxml", entry.bad);
        if !bad
            .report
            .diagnostics()
            .iter()
            .any(|d| matches(entry, &d.code))
        {
            failures.push(format!(
                "{}: erroneous example does not emit the code: {:?}",
                entry.code,
                bad.report
                    .diagnostics()
                    .iter()
                    .map(|d| &d.code)
                    .collect::<Vec<_>>()
            ));
        }
        // An agent copies the corrected example: it must be clean, with no
        // finding of any severity, not merely free of this code.
        let fixed = validator.validate_source(SOURCE, "fixed.scxml", entry.fixed);
        let remaining = fixed
            .report
            .diagnostics()
            .iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>();
        if !remaining.is_empty() {
            failures.push(format!(
                "{}: corrected example is not clean: {remaining:?}",
                entry.code
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
