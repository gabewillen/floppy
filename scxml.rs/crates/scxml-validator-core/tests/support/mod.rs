//! Test adapter shared by the integration tests: the ECMAScript data
//! model's parse-only side-effect analysis in the validator's terms, as
//! `scx` links it.

use scxml_validator_core::ExternalScripts;
use scxml_validator_core::{
    EffectCause, EffectCheck, EffectFinding, EffectFragment, EffectKind, EffectRole, EffectSpan,
};

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
