//! The validator and compiler composed exactly as `scxml-cli` composes them:
//! the bundled W3C SCXML schema and both parse-only expression checkers.

use std::sync::Arc;

use scxml_compiler_core::{CompileInput, CompileOutput, Compiler, ScriptResolution};
use scxml_data_model_ecmascript::{SyntaxKind, check_syntax};
use scxml_syntax::SourceId;
mod effects;

use scxml_validator_core::{
    EffectCheck, EffectFragment, ExpressionCheck, ExpressionKind, ExpressionSyntaxChecker,
    ExternalScripts, MemorySchemaResolver, Report, SyntaxIssue, Validator,
};
use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

/// Parse-only ECMAScript checker. Unoptimized test builds need a large
/// native stack per Boa parser level, so each check runs on its own thread.
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
        issues(result.map_err(|_| ()), |issue| SyntaxIssue {
            start: issue.start,
            end: issue.end,
            message: issue.message,
        })
    }

    fn check_effects(
        &self,
        fragments: &[EffectFragment<'_>],
        scripts: ExternalScripts,
    ) -> EffectCheck {
        effects::ecmascript_effects(fragments, scripts)
    }
}

/// Parse-only `XPath` checker.
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
        issues(
            scxml_data_model_xpath::check_syntax(kind, source).map_err(|_| ()),
            |issue| SyntaxIssue {
                start: issue.start,
                end: issue.end,
                message: issue.message,
            },
        )
    }

    fn check_effects(
        &self,
        _fragments: &[EffectFragment<'_>],
        _scripts: ExternalScripts,
    ) -> EffectCheck {
        EffectCheck::Checked(Vec::new())
    }
}

fn issues<T>(result: Result<Vec<T>, ()>, map: impl Fn(T) -> SyntaxIssue) -> ExpressionCheck {
    match result {
        Ok(found) if found.is_empty() => ExpressionCheck::Valid,
        Ok(found) => ExpressionCheck::Invalid(found.into_iter().map(map).collect()),
        Err(()) => ExpressionCheck::NotChecked,
    }
}

type CliValidator = Validator<BundledSchemaResolver<MemorySchemaResolver>, XsdSchemaBackend>;

/// The validator `scxml-cli validate` runs.
pub fn validator() -> CliValidator {
    Validator::new(
        BundledSchemaResolver::new(MemorySchemaResolver::new()),
        XsdSchemaBackend,
    )
    .with_expression_checker(Arc::new(EcmaScriptChecker))
    .with_expression_checker(Arc::new(XPathChecker))
}

/// The report `scxml-cli validate` produces for `source`.
pub fn validate(source: &str) -> Report {
    validator()
        .validate_source(SOURCE, "case.scxml", source)
        .report
}

/// `scxml-cli compile` without namespace, script, or service packages, in
/// the default compile-time script resolution.
pub fn compile(source: &str) -> CompileOutput {
    Compiler::new(validator()).compile(CompileInput {
        source_name: "case.scxml".into(),
        source: source.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: Vec::new(),
        script_resolution: ScriptResolution::Compile,
        event_schemas: std::collections::BTreeMap::new(),
    })
}
