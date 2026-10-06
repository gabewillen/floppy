//! Native ECMAScript and `XPath` expression syntax capabilities for `validate`
//! and `compile`.
//!
//! The CLI is a native composition root, so it links each data-model crate's
//! parse-only `check_syntax` directly and passes them to the validator as
//! explicit [`ExpressionSyntaxChecker`]s. Nothing is evaluated. Fragments
//! the checker declines (size, NUL, or nesting bounds, which the data model
//! applies identically at evaluation) are the `SCXML040` error, so `compile`
//! refuses them rather than guessing. The ECMAScript data model's
//! parse-only `check_effects` analyzes conditions and `<log expr>` for side
//! effects (`SCXML074`/`SCXML075`); `XPath` expressions have none.

use scxml_data_model_ecmascript::{SyntaxKind, check_syntax};
use scxml_validator_core::{
    EffectCause, EffectCheck, EffectFinding, EffectFragment, EffectKind, EffectRole, EffectSpan,
    ExpressionCheck, ExpressionKind, ExpressionSyntaxChecker, ExternalScripts, SyntaxIssue,
};

/// Stack reserved for a validation run that parses ECMAScript fragments.
///
/// Boa's recursive-descent parser uses roughly 25 KiB per nesting level in
/// optimized builds and several times more unoptimized; the checker bounds
/// nesting at 128 structural units (16 bracket levels), so 64 MiB leaves a
/// wide margin in every profile.
pub const VALIDATION_STACK_BYTES: usize = 64 * 1024 * 1024;

/// Parse-only ECMAScript checker for `datamodel="ecmascript"` documents.
#[derive(Clone, Copy, Debug, Default)]
pub struct EcmaScriptSyntax;

impl ExpressionSyntaxChecker for EcmaScriptSyntax {
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
        match check_syntax(kind, source) {
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
        ecmascript_effects(fragments, scripts)
    }
}

/// The ECMAScript data model's side-effect analysis in the validator's
/// terms.
#[must_use]
pub fn ecmascript_effects(
    fragments: &[EffectFragment<'_>],
    scripts: ExternalScripts,
) -> EffectCheck {
    use scxml_data_model_ecmascript as model;
    let request: Vec<model::EffectFragment<'_>> = fragments
        .iter()
        .map(|fragment| model::EffectFragment {
            role: match fragment.role {
                EffectRole::Condition => model::EffectRole::Condition,
                EffectRole::Log => model::EffectRole::Log,
                EffectRole::Script => model::EffectRole::Script,
                EffectRole::Data(id) => model::EffectRole::Data(id),
                EffectRole::Value => model::EffectRole::Value,
                EffectRole::Location => model::EffectRole::Location,
            },
            source: fragment.source,
        })
        .collect();
    let Ok(findings) = model::check_effects(&request, scripts) else {
        return EffectCheck::NotChecked;
    };
    let kind = |kind: model::EffectKind| match kind {
        model::EffectKind::Assignment => EffectKind::Assignment,
        model::EffectKind::Update => EffectKind::Update,
        model::EffectKind::Deletion => EffectKind::Deletion,
        model::EffectKind::MutatingCall => EffectKind::MutatingCall,
        model::EffectKind::Random => EffectKind::Random,
        model::EffectKind::FunctionCall => EffectKind::FunctionCall,
        model::EffectKind::UnknownCall => EffectKind::UnknownCall,
        model::EffectKind::DynamicCode => EffectKind::DynamicCode,
        model::EffectKind::ImplicitCall => EffectKind::ImplicitCall,
        model::EffectKind::UnknownWrite => EffectKind::UnknownWrite,
        model::EffectKind::DynamicScope => EffectKind::DynamicScope,
        model::EffectKind::UnseenScript => EffectKind::UnseenScript,
        model::EffectKind::TooComplex => EffectKind::TooComplex,
        model::EffectKind::SystemVariableWrite => EffectKind::SystemVariableWrite,
    };
    let span = |span: model::EffectSpan| EffectSpan {
        fragment: span.fragment,
        start: span.start,
        end: span.end,
    };
    EffectCheck::Checked(
        findings
            .into_iter()
            .map(|finding| EffectFinding {
                certain: finding.certainty == model::Certainty::Certain,
                kind: kind(finding.kind),
                span: span(finding.span),
                subject: finding.subject,
                cause: finding.cause.map(|cause| EffectCause {
                    kind: kind(cause.kind),
                    span: span(cause.span),
                    subject: cause.subject,
                    function: cause.function,
                }),
                operator: finding.operator.map(span),
            })
            .collect(),
    )
}

/// Parse-only `XPath` checker for `datamodel="xpath"` documents.
#[derive(Clone, Copy, Debug, Default)]
pub struct XPathSyntax;

impl ExpressionSyntaxChecker for XPathSyntax {
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

    /// `XPath` 1.0 has no assignment operator and its function library only
    /// computes values, so no `XPath` expression has a side effect.
    fn check_effects(
        &self,
        _fragments: &[EffectFragment<'_>],
        _scripts: ExternalScripts,
    ) -> EffectCheck {
        EffectCheck::Checked(Vec::new())
    }
}
