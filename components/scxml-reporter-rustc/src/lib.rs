//! WebAssembly reporter for deterministic canonical SCXML reports.
//!
//! The component imports no filesystem or network capabilities. It validates
//! the complete canonical input before rendering validator, compiler, or
//! conformance payloads without recomputing producer decisions.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    use scxml_telemetry::{
        ErrorCategory, Module, Operation, SignalEvent, TraceContext, WasmSpanAdapter, WasmTraceHost,
    };

    wit_bindgen::generate!({
        world: "reporter",
        path: "../../crates/scxml-wit/wit/reporter",
    });

    use exports::stateforward::scxml_reporter::reporter_api::Guest;
    use stateforward::scxml_reporter::telemetry as wit_telemetry;
    use stateforward::scxml_reporter::types::{
        ComponentError, ErrorCode, ErrorVersion, ReportOutput, ReportRequest,
    };

    struct Component;
    struct TraceHost;

    static TRACE_HOST: TraceHost = TraceHost;

    impl WasmTraceHost for TraceHost {
        type Span = u64;

        fn start(
            &self,
            _module: Module,
            _operation: Operation,
            parent: Option<&TraceContext>,
        ) -> (Self::Span, Option<TraceContext>) {
            let parent = parent.map(to_wit_context);
            let (span, context) = wit_telemetry::start(
                wit_telemetry::Module::ReporterRustc,
                wit_telemetry::Operation::Render,
                parent.as_ref(),
            );
            (span, context.map(from_wit_context))
        }

        fn end(&self, span: Self::Span, error: Option<ErrorCategory>) {
            wit_telemetry::end(
                span,
                error.map(|category| match category {
                    ErrorCategory::InvalidInput | ErrorCategory::Parse => {
                        wit_telemetry::ErrorCategory::InvalidInput
                    }
                    _ => wit_telemetry::ErrorCategory::Report,
                }),
            );
        }

        fn event(&self, span: &Self::Span, event: SignalEvent) {
            let event = match event {
                SignalEvent::ArtifactAccepted => wit_telemetry::SignalEvent::ArtifactAccepted,
                SignalEvent::RenderCompleted => wit_telemetry::SignalEvent::RenderCompleted,
                SignalEvent::InputAccepted
                | SignalEvent::InputRejected
                | SignalEvent::ExternalReadCompleted
                | SignalEvent::TransformationCompleted
                | SignalEvent::ArtifactProduced
                | SignalEvent::ArtifactReused
                | SignalEvent::ScxmlEventReceived
                | SignalEvent::ScxmlEventEmitted
                | SignalEvent::ClockScheduled
                | SignalEvent::ClockCancelled
                | SignalEvent::ClockDueDrained
                | SignalEvent::ClockAdvanced
                | SignalEvent::ClockRejected
                | SignalEvent::IoProcessorTypeUriReturned
                | SignalEvent::IoProcessorSendAccepted
                | SignalEvent::IoProcessorSendRejected
                | SignalEvent::IoProcessorCancelCompleted
                | SignalEvent::IoProcessorCancelRejected
                | SignalEvent::IoProcessorFailureReported
                | SignalEvent::RecorderIdExhausted => return,
            };
            wit_telemetry::event(*span, event);
        }
    }

    fn to_wit_context(context: &TraceContext) -> wit_telemetry::TraceContext {
        wit_telemetry::TraceContext {
            traceparent: context.traceparent.clone(),
            tracestate: context.tracestate.clone(),
        }
    }

    fn from_wit_context(context: wit_telemetry::TraceContext) -> TraceContext {
        TraceContext {
            traceparent: context.traceparent,
            tracestate: context.tracestate,
        }
    }

    impl Guest for Component {
        fn render(request: ReportRequest) -> Result<ReportOutput, ComponentError> {
            let parent = request.trace_context.as_ref().map(|context| TraceContext {
                traceparent: context.traceparent.clone(),
                tracestate: context.tracestate.clone(),
            });
            let span = WasmSpanAdapter::new(
                &TRACE_HOST,
                Module::ReporterRustcComponent,
                Operation::ReporterRustcRender,
                parent.as_ref(),
            );
            match super::render(&request.artifact) {
                Ok(bytes) => {
                    span.event(SignalEvent::ArtifactAccepted);
                    span.event(SignalEvent::RenderCompleted);
                    Ok(ReportOutput {
                        media_type: super::MEDIA_TYPE.into(),
                        bytes,
                    })
                }
                Err(code) => {
                    span.fail(match code {
                        super::RenderErrorCode::ArtifactTooLarge
                        | super::RenderErrorCode::InvalidArtifact
                        | super::RenderErrorCode::UnsupportedVersion
                        | super::RenderErrorCode::UnsupportedKind
                        | super::RenderErrorCode::InconsistentArtifact => {
                            ErrorCategory::InvalidInput
                        }
                    });
                    Err(component_error(code))
                }
            }
        }
    }

    fn component_error(code: super::RenderErrorCode) -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code: match code {
                super::RenderErrorCode::ArtifactTooLarge => ErrorCode::ArtifactTooLarge,
                super::RenderErrorCode::InvalidArtifact => ErrorCode::InvalidArtifact,
                super::RenderErrorCode::UnsupportedVersion => ErrorCode::UnsupportedVersion,
                super::RenderErrorCode::UnsupportedKind => ErrorCode::UnsupportedKind,
                super::RenderErrorCode::InconsistentArtifact => ErrorCode::InconsistentArtifact,
            },
        }
    }

    export!(Component);
}

use scxml_reporting::{ArtifactError, RustcRenderer, decode_canonical};

/// Media type of every successful rendering.
pub const MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// Closed render failure, mirroring the WIT reporter `error-code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderErrorCode {
    /// The artifact or its rendering exceeded a documented bound.
    ArtifactTooLarge,
    /// The bytes were not the canonical envelope schema (including unknown fields).
    InvalidArtifact,
    /// The envelope or payload version is unsupported.
    UnsupportedVersion,
    /// The envelope kind is unsupported.
    UnsupportedKind,
    /// Summaries disagree with the payload they describe.
    InconsistentArtifact,
}

/// Classifies one canonical-artifact failure into its closed WIT code.
#[must_use]
pub fn error_code(error: &ArtifactError) -> RenderErrorCode {
    match error {
        ArtifactError::LimitExceeded(_) => RenderErrorCode::ArtifactTooLarge,
        ArtifactError::UnsupportedVersion(_) | ArtifactError::UnsupportedPayloadVersion { .. } => {
            RenderErrorCode::UnsupportedVersion
        }
        ArtifactError::UnsupportedKind(_) => RenderErrorCode::UnsupportedKind,
        ArtifactError::InconsistentCounts { .. }
        | ArtifactError::InconsistentCompilerStatus
        | ArtifactError::NonCanonicalPackageRecipe
        | ArtifactError::InconsistentConformanceSummary
        | ArtifactError::ConformanceDigestMismatch => RenderErrorCode::InconsistentArtifact,
        ArtifactError::InvalidJson(_)
        | ArtifactError::InvalidConformanceIdentity(_)
        | ArtifactError::InvalidConformanceCase(_)
        | ArtifactError::EmptyCode
        | ArtifactError::InvalidSpan(_)
        | ArtifactError::OverlappingEdits
        | ArtifactError::NonCanonicalOrdering
        | ArtifactError::NonCanonicalJson => RenderErrorCode::InvalidArtifact,
    }
}

/// Validates the complete canonical artifact, then renders it.
///
/// Consumes only producer bytes and never recomputes producer decisions.
/// Output is byte-deterministic for identical input.
///
/// # Errors
///
/// Returns the closed [`RenderErrorCode`] for rejected or unrenderable input;
/// no partial output is produced.
pub fn render(artifact: &[u8]) -> Result<Vec<u8>, RenderErrorCode> {
    decode_canonical(artifact)
        .and_then(|artifact| {
            RustcRenderer::default()
                .render(&artifact)
                .map(String::into_bytes)
        })
        .map_err(|error| error_code(&error))
}

/// Native marker for workspace tests.
#[must_use]
pub fn is_guest_component() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_reporting::{
        ArtifactEnvelope, ByteSpan, Diagnostic, DiagnosticCategory, Label, MAX_CANONICAL_BYTES,
        Severity, encode_canonical,
    };

    fn canonical_validator_report() -> Vec<u8> {
        let artifact = ArtifactEnvelope::validator(
            "machine.scxml",
            "<scxml>\n  <state id=\"bad\"/>\n</scxml>\n",
            vec![Diagnostic {
                severity: Severity::Error,
                code: "SCXML999".into(),
                category: DiagnosticCategory::Semantic,
                message: "state is invalid".into(),
                primary: Label {
                    span: ByteSpan { start: 10, end: 15 },
                    message: Some("here".into()),
                },
                secondary: Vec::new(),
                notes: Vec::new(),
                help: None,
                spec_references: Vec::new(),
                fixes: Vec::new(),
            }],
        )
        .expect("fixture validator artifact satisfies canonical invariants");
        encode_canonical(&artifact).expect("fixture artifact encodes canonically")
    }

    /// Re-encodes a mutated envelope with canonical (sorted-key) JSON.
    fn mutated(mutate: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>)) -> Vec<u8> {
        let mut value: serde_json::Value = serde_json::from_slice(&canonical_validator_report())
            .expect("canonical report is JSON");
        let object = value
            .as_object_mut()
            .expect("canonical report is a JSON object");
        mutate(object);
        serde_json::to_vec(&value).expect("mutated report serializes")
    }

    #[test]
    fn accepted_canonical_report_renders_deterministically() {
        let bytes = canonical_validator_report();
        let first = render(&bytes).expect("canonical report renders");
        let second = render(&bytes).expect("canonical report renders again");
        assert_eq!(first, second, "rendering must be byte-deterministic");
        assert!(
            String::from_utf8(first)
                .expect("rustc output is UTF-8")
                .contains("SCXML999")
        );
    }

    #[test]
    fn unknown_version_kind_and_field_fail_closed() {
        assert_eq!(
            render(&mutated(|object| {
                object.insert("version".into(), "scxml.report/v2".into());
            })),
            Err(RenderErrorCode::UnsupportedVersion)
        );
        assert_eq!(
            render(&mutated(|object| {
                object.insert("kind".into(), "unknown".into());
            })),
            Err(RenderErrorCode::UnsupportedKind)
        );
        assert_eq!(
            render(&mutated(|object| {
                object.insert("unexpected".into(), serde_json::Value::Bool(true));
            })),
            Err(RenderErrorCode::InvalidArtifact)
        );
        assert_eq!(render(b"not json"), Err(RenderErrorCode::InvalidArtifact));
    }

    #[test]
    fn oversized_artifact_is_rejected_before_parsing() {
        let oversized = vec![b' '; MAX_CANONICAL_BYTES + 1];
        assert_eq!(render(&oversized), Err(RenderErrorCode::ArtifactTooLarge));
    }
}
