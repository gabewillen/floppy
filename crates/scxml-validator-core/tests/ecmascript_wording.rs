//! `SCXML038`/`SCXML039` wording never exposes the ECMAScript parser's own
//! messages (docs/validator-rules.md: checker messages are rewritten into
//! project-owned wording that echoes the source text). Each fragment below
//! is a mistake agents make; each is checked with the real Boa-backed
//! checker, composed as `scxml-cli validate` composes it.

use scxml_validator_core::ExternalScripts;
use std::sync::Arc;

use scxml_data_model_ecmascript::{SyntaxKind, check_syntax};
use scxml_syntax::SourceId;
mod support;

use scxml_validator_core::{
    EffectCheck, EffectFragment, ExpressionCheck, ExpressionKind, ExpressionSyntaxChecker,
    MemorySchemaResolver, SchemaMode, SyntaxIssue, Validator,
};
use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

/// The source id of the documents these tests parse.
const SOURCE: SourceId = SourceId::new(1);

/// Parse-only ECMAScript checker, as the CLI links it (on a large stack:
/// unoptimized Boa needs one per parser level).
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

/// Value expressions (`<log expr>`) agents write by mistake.
const VALUES: &[&str] = &[
    "# heading",
    "## Retry loop",
    "a # b",
    "x @ y",
    "Starting…",
    "😀 hi",
    "Error: failed",
    "\\x",
    "x.#y",
    "`total",
    "it's done",
    "list.0",
    "list.0.name",
    "// just a comment",
    "x ~~ y",
    "{a: }",
    "1e",
    "0x",
    "x ? 1",
    "…",
    "a.b.",
    "f(,)",
    "@decorator",
    "x => ",
    "#!/bin/sh",
];

/// Raw parser vocabulary that must never reach a message.
const RAW_PHRASES: &[&str] = &[
    "Abrupt end",
    "Expecting",
    "private identifier",
    "unexpected '",
    "Unexpected",
    "primary expression",
    "ParseError",
    "invalid declaration scope",
];

#[test]
fn ecmascript_findings_never_expose_raw_parser_messages() {
    let validator = Validator::new(
        BundledSchemaResolver::new(MemorySchemaResolver::new()),
        XsdSchemaBackend,
    )
    .with_schema_mode(SchemaMode::Strict)
    .with_expression_checker(Arc::new(EcmaScriptChecker));
    let mut leaks = Vec::new();
    for value in VALUES {
        let escaped = value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('"', "&quot;");
        let source = format!(
            "<scxml xmlns=\"http://www.w3.org/2005/07/scxml\" version=\"1.0\" datamodel=\"ecmascript\" initial=\"a\"><datamodel><data id=\"list\" expr=\"[]\"/><data id=\"x\" expr=\"1\"/><data id=\"y\" expr=\"1\"/></datamodel><state id=\"a\"><onentry><log expr=\"{escaped}\"/><assign location=\"{escaped}\" expr=\"1\"/><script>{escaped}</script></onentry><transition event=\"go\" target=\"b\"/></state><final id=\"b\"/></scxml>"
        );
        let outcome = validator.validate_source(SOURCE, "wording.scxml", source);
        let findings: Vec<_> = outcome
            .report
            .diagnostics()
            .iter()
            .filter(|diagnostic| matches!(diagnostic.code.as_str(), "SCXML038" | "SCXML039"))
            .collect();
        if findings.is_empty() {
            leaks.push(format!("{value:?}: no SCXML038/SCXML039 finding"));
        }
        for diagnostic in findings {
            for phrase in RAW_PHRASES {
                if diagnostic.message.contains(phrase) {
                    leaks.push(format!(
                        "{value:?}: {} message exposes `{phrase}`: {}",
                        diagnostic.code, diagnostic.message
                    ));
                }
            }
            if diagnostic.help.as_deref()
                == Some("fix the ecmascript syntax at the marked position")
            {
                leaks.push(format!(
                    "{value:?}: {} has only the generic help",
                    diagnostic.code
                ));
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}
