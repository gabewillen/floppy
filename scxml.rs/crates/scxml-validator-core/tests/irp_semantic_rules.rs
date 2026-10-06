//! Official W3C IRP corpus regression for graph and expression rules.
//!
//! Every prepared IRP document (ECMAScript and `XPath` corpora) is valid SCXML
//! by definition, so a new graph or expression ERROR on it is a false
//! positive unless the test deliberately embeds an invalid expression to
//! exercise `error.execution`. Those deliberate cases are enumerated below
//! with the exact code and count they must produce, so both false positives
//! and silently lost true positives fail this test, and the warning and note
//! inventory of each corpus is pinned exactly.

use scxml_validator_core::ExternalScripts;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use scxml_data_model_ecmascript::{SyntaxKind, check_syntax};
use scxml_syntax::{Document, SourceId};
mod support;

use scxml_validator_core::{
    EffectCheck, EffectFragment, ExpressionCheck, ExpressionKind, ExpressionSyntaxChecker,
    MemorySchemaResolver, Report, SchemaMode, SchemaSource, SchemaValidator, Severity, SyntaxIssue,
    Validator,
};

/// Test adapter over the `XPath` component's native parse-only check.
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

/// Test adapter over the ECMAScript component's native parse-only check.
///
/// Unoptimized test builds need far more native stack per Boa parser level
/// than release builds, so each check runs on a thread with a large stack.
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

#[derive(Debug)]
struct NoSchemaBackend;

impl SchemaValidator for NoSchemaBackend {
    fn validate(
        &self,
        _document: &Document,
        _schemas: &[SchemaSource],
    ) -> Result<Report, scxml_validator_core::SchemaBackendError> {
        Ok(Report::new())
    }
}

/// Exact warning and note inventory over the ECMAScript corpus, by code.
///
/// Every entry is a reviewed true finding on valid SCXML; any other lint is a
/// false positive on idiomatic code, and a changed count must be re-reviewed.
/// For example, `SCXML024` ("may never exit") is not reported for a state
/// that an ancestor transition or a sibling `<parallel>` region exits, so the
/// corpus has none.
const ECMASCRIPT_WARNINGS: &[(&str, usize)] = &[
    // compound `<scxml>`/`<state>` entry relies on document order
    ("SCXML023", 16),
    // states whose only exits are conditional and eventless
    ("SCXML025", 3),
    // states the tests never enter (reachability over-approximates)
    ("SCXML029", 20),
    // duplicate `done.state.s0` transitions in tests 343 and 488
    ("SCXML034", 2),
    // empty `<log expr>` in test 307
    ("SCXML042", 2),
    // `In('sh1')` of a `<history>` in test 580, which asserts that a history
    // pseudo-state never becomes active
    ("SCXML058", 3),
    // `cond="++var1==2"` in test 444 increments `var1`: a side effect in a
    // condition, which §5.9.1 says conformant documents SHOULD NOT have;
    // the test requires it to run (Appendix B's MUST yields to the IRP,
    // docs/spec-errata.md), so it is a warning
    ("SCXML076", 1),
];

/// ECMAScript IRP documents that deliberately contain an unparsable
/// expression or an unassignable location to exercise `error.execution`
/// (true positives): file name, code, and exact count. SCXML defines the
/// run-time outcome of each (5.9.4, 5.9.2, 4.6: `error.execution`), so each
/// is a warning and the document compiles.
const ECMASCRIPT_DELIBERATE: &[(&str, &str, usize)] = &[
    // `expr="return"` / `cond="return"`: an illegal expression by design.
    ("test156.scxml", "SCXML038", 1),
    ("test277.scxml", "SCXML038", 1),
    ("test309.scxml", "SCXML038", 1),
    ("test312.scxml", "SCXML038", 1),
    ("test313.scxml", "SCXML038", 1),
    ("test314.scxml", "SCXML038", 1),
    ("test344.scxml", "SCXML038", 1),
    ("test487.scxml", "SCXML038", 1),
    ("test488.scxml", "SCXML038", 1),
    ("test528.scxml", "SCXML038", 1),
    // `<foreach item="'continue'">`: an illegal item location by design.
    ("test152.scxml", "SCXML039", 1),
    ("test457.scxml", "SCXML039", 1),
    // `namelist="&#34;foo"`: an illegal namelist location by design.
    ("test553.scxml", "SCXML039", 1),
    ("test554.scxml", "SCXML039", 1),
    // Upstream 436 fixes `datamodel="null"`, but the template logs its
    // outcomes with a value expression (`expr="'pass'"`), which SCXML 1.0
    // §B.1.4 rules out ("There is no value expression language."). The case
    // is declared not applicable to this corpus.
    ("test436.scxml", "SCXML079", 2),
];

/// Exact warning and note inventory over the `XPath` corpus, by code. The
/// graph findings are the ECMAScript corpus's (the same machines).
const XPATH_WARNINGS: &[(&str, usize)] = &[
    // compound `<scxml>`/`<state>` entry relies on document order
    ("SCXML023", 16),
    // states whose only exits are conditional and eventless
    ("SCXML025", 3),
    // states the tests never enter (reachability over-approximates)
    ("SCXML029", 20),
    // duplicate `done.state.s0` transitions in tests 343 and 488
    ("SCXML034", 2),
    // empty `<log expr>` in test 307
    ("SCXML042", 2),
    // `In('sh1')` of a `<history>` in test 580, which asserts that a history
    // pseudo-state never becomes active
    ("SCXML058", 3),
    // `cond="++var1==2"` in test 444 increments `var1`: a side effect in a
    // condition, which §5.9.1 says conformant documents SHOULD NOT have;
    // the test requires it to run (Appendix B's MUST yields to the IRP,
    // docs/spec-errata.md), so it is a warning
    ("SCXML076", 1),
    // `location="foo.bar.baz "`: an illegal location by design (tests 286,
    // 298, 311, 331, 343, 401, 402); it selects no `<datamodel>` child
    ("SCXML057", 7),
];

/// `XPath` IRP documents that deliberately contain an invalid expression or
/// location to exercise `error.execution` (true positives): file name, code,
/// and exact count, all warnings (their run-time outcome is `error.execution`).
/// Tests 457 and 553/554 keep ECMAScript bodies in the `XPath` corpus, so
/// they match the ECMAScript entries.
const XPATH_DELIBERATE: &[(&str, &str, usize)] = &[
    // `<data expr="(">`: an illegal value expression by design.
    ("test152.scxml", "SCXML038", 1),
    // `expr="!1"` / `cond="==!*"`: an illegal expression by design.
    ("test156.scxml", "SCXML038", 1),
    // `location="$Var5"`: "assign an illegal value to a non-existent var".
    ("test156.scxml", "SCXML056", 1),
    // The invoked child reads `$Var2`, which it deliberately does not
    // declare ("Var2 is not defined in invoked child's datamodel").
    ("test245.scxml", "SCXML056", 2),
    // `$Var1` would be declared by a script; the W3C marks 302 and 304 not
    // applicable to the XPath data model, which has no scripting.
    ("test302.scxml", "SCXML056", 1),
    ("test304.scxml", "SCXML056", 1),
    ("test277.scxml", "SCXML038", 1),
    ("test309.scxml", "SCXML038", 1),
    ("test312.scxml", "SCXML038", 1),
    ("test313.scxml", "SCXML038", 1),
    ("test314.scxml", "SCXML038", 1),
    ("test344.scxml", "SCXML038", 1),
    ("test487.scxml", "SCXML038", 1),
    ("test488.scxml", "SCXML038", 1),
    ("test528.scxml", "SCXML038", 1),
    // `<foreach item="'continue'">`: an illegal item location by design.
    ("test457.scxml", "SCXML039", 1),
    // Upstream 456 fixes `datamodel="ecmascript"`, but the XPath template
    // writes its `conf:idVal` guard as XPath (`$Var1/text() =1`), which is
    // not ECMAScript. The case is declared not applicable to XPath
    // (errata/xpath.json) and prepared from the unmodified upstream input.
    ("test456.scxml", "SCXML038", 1),
    // `namelist="&#34;foo&#34;"`: an illegal namelist location by design.
    ("test553.scxml", "SCXML039", 1),
    ("test554.scxml", "SCXML039", 1),
    // Upstream 436 fixes `datamodel="null"`, but the template logs its
    // outcomes with a value expression, which SCXML 1.0 §B.1.4 rules out
    // ("There is no value expression language."); the case is declared not
    // applicable to XPath (errata/xpath.json).
    ("test436.scxml", "SCXML079", 2),
];

/// The expression findings pinned per document rather than by count.
const DELIBERATE_CODES: [&str; 4] = ["SCXML038", "SCXML039", "SCXML056", "SCXML079"];

fn corpus_root(data_model: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/w3c-irp/prepared")
        .join(data_model)
}

fn documents(data_model: &str) -> Vec<PathBuf> {
    let root = corpus_root(data_model);
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("corpus.json")).expect("prepared IRP manifest is checked in"),
    )
    .expect("prepared IRP manifest is JSON");
    let mut paths = BTreeSet::new();
    for case in manifest["cases"].as_array().expect("cases array") {
        // Cases without a prepared SCXML source (for example 513, whose
        // official artifact is not an SCXML document) have nothing to validate.
        if let Some(source) = case["source"].as_str() {
            paths.insert(root.join(source));
        }
        for dependency in case["dependencies"].as_array().expect("dependencies array") {
            let dependency = dependency.as_str().expect("dependency path");
            if Path::new(dependency)
                .extension()
                .is_some_and(|extension| extension == "scxml")
            {
                paths.insert(root.join(dependency));
            }
        }
    }
    paths.into_iter().collect()
}

/// Validates every prepared document of one corpus with both expression
/// checkers linked (as `scxml-cli` does) and compares the findings with the
/// pinned inventory.
fn check_corpus(
    data_model: &str,
    expected_warnings: &[(&str, usize)],
    expected_deliberate: &[(&str, &str, usize)],
) {
    let validator = Validator::new(MemorySchemaResolver::default(), NoSchemaBackend)
        .with_schema_mode(SchemaMode::Off)
        .with_expression_checker(Arc::new(EcmaScriptChecker))
        .with_expression_checker(Arc::new(XPathChecker));
    let mut warnings: BTreeMap<String, usize> = BTreeMap::new();
    let mut errors: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut deliberate: BTreeMap<(String, String), usize> = BTreeMap::new();
    let paths = documents(data_model);
    assert!(
        paths.len() >= 200,
        "{data_model} corpus must be present: {}",
        paths.len()
    );
    for (index, path) in paths.iter().enumerate() {
        let text = std::fs::read_to_string(path).expect("prepared IRP document is UTF-8");
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let id = u32::try_from(index + 1).expect("corpus index fits u32");
        let outcome = validator.validate_source(SourceId::new(id), name.clone(), text);
        for diagnostic in outcome.report.diagnostics() {
            eprintln!(
                "{data_model}/{name}: {} {:?} {}",
                diagnostic.code, diagnostic.severity, diagnostic.message
            );
            if diagnostic.severity == Severity::Error {
                *errors
                    .entry((name.clone(), diagnostic.code.clone()))
                    .or_default() += 1;
            } else if DELIBERATE_CODES.contains(&diagnostic.code.as_str()) {
                *deliberate
                    .entry((name.clone(), diagnostic.code.clone()))
                    .or_default() += 1;
            } else {
                *warnings.entry(diagnostic.code.clone()).or_default() += 1;
            }
        }
    }
    assert!(
        errors.is_empty(),
        "{data_model} IRP documents are conforming, so no error may be reported: {errors:?}"
    );
    let expected: BTreeMap<(String, String), usize> = expected_deliberate
        .iter()
        .map(|(name, code, count)| (((*name).to_owned(), (*code).to_owned()), *count))
        .collect();
    assert_eq!(
        deliberate, expected,
        "{data_model} IRP expression findings changed: a new one is a false positive unless the test deliberately embeds it, and a lost one is a silently dropped true positive"
    );
    let expected: BTreeMap<String, usize> = expected_warnings
        .iter()
        .map(|(code, count)| ((*code).to_owned(), *count))
        .collect();
    assert_eq!(
        warnings, expected,
        "{data_model} IRP warning inventory changed; review every new lint"
    );
}

#[test]
fn official_ecmascript_irp_documents_have_a_pinned_inventory_without_false_positives() {
    check_corpus("ecmascript", ECMASCRIPT_WARNINGS, ECMASCRIPT_DELIBERATE);
}

#[test]
fn official_xpath_irp_documents_have_a_pinned_inventory_without_false_positives() {
    check_corpus("xpath", XPATH_WARNINGS, XPATH_DELIBERATE);
}
