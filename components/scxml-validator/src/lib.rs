//! SCXML validator WebAssembly guest.
//!
//! All schema/package bytes arrive in the request. The guest has no filesystem
//! or network imports and returns the canonical versioned validator artifact as
//! deterministic JSON bytes.

#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]
#![cfg_attr(
    target_arch = "wasm32",
    allow(
        unsafe_code,
        reason = "wit-bindgen generated ABI glue only; scripts/verify-component-authored-unsafe.py rejects authored unsafe"
    )
)]

#[cfg(any(target_arch = "wasm32", test))]
use scxml_reporting::{ByteSpan, Diagnostic, DiagnosticCategory, Label, Severity};
#[cfg(any(target_arch = "wasm32", test))]
use scxml_validator_core::{ExtensionPackageManifest, SCXML_NAMESPACE, SchemaSource};

#[cfg(any(target_arch = "wasm32", test))]
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

#[cfg(any(target_arch = "wasm32", test))]
struct SchemaBinding<'a> {
    namespace_uri: &'a str,
    system_id: &'a str,
    bytes: &'a [u8],
}

#[cfg(any(target_arch = "wasm32", test))]
struct PackageBinding<'a> {
    namespace_uri: &'a str,
    package_version: &'a str,
    extension_artifact: &'a str,
    schema_artifact: &'a str,
    schema_sha256: &'a str,
    wit_world: &'a str,
    wit_version: &'a str,
    env_slots: &'a [String],
    http_origins: &'a [String],
}

#[cfg(any(target_arch = "wasm32", test))]
fn package_diagnostics(
    schemas: &[SchemaBinding<'_>],
    packages: &[PackageBinding<'_>],
    used_namespaces: &[String],
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for namespace_uri in used_namespaces
        .iter()
        .filter(|namespace_uri| namespace_uri.as_str() != SCXML_NAMESPACE)
        .filter(|namespace_uri| namespace_uri.as_str() != XML_NAMESPACE)
        // The host's `env` and `event` attribute vocabularies have no package.
        .filter(|namespace_uri| namespace_uri.as_str() != scxml_validator_core::ENV_NAMESPACE_URI)
        .filter(|namespace_uri| namespace_uri.as_str() != scxml_validator_core::EVENT_NAMESPACE_URI)
    {
        let verification = verify_package_binding(namespace_uri, schemas, packages);
        if let Err(message) = verification {
            diagnostics.push(package_diagnostic(message));
        }
    }
    for package in packages.iter().filter(|package| {
        package.namespace_uri == SCXML_NAMESPACE
            || package.namespace_uri == XML_NAMESPACE
            || !used_namespaces
                .iter()
                .any(|namespace_uri| namespace_uri == package.namespace_uri)
    }) {
        if let Err(message) = verify_package_binding(package.namespace_uri, schemas, packages) {
            diagnostics.push(package_diagnostic(message));
        }
    }
    diagnostics
}

#[cfg(any(target_arch = "wasm32", test))]
fn verify_package_binding(
    namespace_uri: &str,
    schemas: &[SchemaBinding<'_>],
    packages: &[PackageBinding<'_>],
) -> Result<(), String> {
    let schema = schemas
        .iter()
        .find(|schema| schema.namespace_uri == namespace_uri)
        .ok_or_else(|| {
            format!("extension namespace `{namespace_uri}` has no exact schema resource binding")
        })?;
    let package = packages
        .iter()
        .find(|package| package.namespace_uri == namespace_uri)
        .ok_or_else(|| {
            format!("extension namespace `{namespace_uri}` has no exact package manifest binding")
        })?;
    let manifest = ExtensionPackageManifest {
        namespace_uri: package.namespace_uri.to_owned(),
        package_version: package.package_version.to_owned(),
        extension_artifact: package.extension_artifact.to_owned(),
        schema_artifact: package.schema_artifact.to_owned(),
        schema_sha256: package.schema_sha256.to_owned(),
        wit_world: package.wit_world.to_owned(),
        wit_version: package.wit_version.to_owned(),
        env_slots: package.env_slots.to_vec(),
        http_origins: package.http_origins.to_vec(),
        // The WIT package record carries no WASI permissions: validation
        // only checks the schema binding and never links the component.
    };
    manifest.verify_schema(&SchemaSource::new(schema.system_id, schema.bytes))
}

#[cfg(any(target_arch = "wasm32", test))]
fn package_diagnostic(message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: "PKG001".into(),
        category: DiagnosticCategory::Package,
        message,
        primary: Label {
            span: ByteSpan { start: 0, end: 0 },
            message: Some("extension package request".into()),
        },
        secondary: Vec::new(),
        notes: vec!["package/schema lookup uses exact namespace URIs".into()],
        help: Some("supply package metadata matching the schema resource".into()),
        spec_references: Vec::new(),
        fixes: Vec::new(),
    }
}

#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::same_length_and_capacity,
    reason = "generated canonical-ABI ownership transfer"
)]
mod guest {
    use std::collections::BTreeSet;

    use scxml_reporting::{
        ArtifactEnvelope, ByteSpan, Diagnostic, Fix, Label, TextEdit, encode_canonical,
    };
    use scxml_telemetry::{
        ErrorCategory, Module, Operation, SignalEvent, TraceContext, WasmSpanAdapter, WasmTraceHost,
    };
    use scxml_validator_core::{
        ComponentRequestBoundary, EffectCause, EffectCheck, EffectFinding, EffectFragment,
        EffectKind, EffectRole, EffectSpan, ExpressionCheck, ExpressionKind,
        ExpressionSyntaxChecker, ExternalScripts, MemorySchemaResolver, SchemaMode, SchemaSource,
        SyntaxIssue, Validator,
    };
    use scxml_validator_xsd::{BundledSchemaResolver, XsdSchemaBackend};

    wit_bindgen::generate!({
        world: "validator",
        path: "../../crates/scxml-wit/wit/validator",
    });

    use exports::stateforward::scxml_validator::validator_api::Guest;
    use stateforward::scxml_validator::expression_syntax as wit_syntax;
    use stateforward::scxml_validator::telemetry as wit_telemetry;
    use stateforward::scxml_validator::types::{
        ComponentError, ErrorCode, ErrorVersion, SchemaPolicy, ValidationRequest,
    };

    /// The source id of the one document a validation reads: every span
    /// points into it.
    const DOCUMENT_SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

    struct Component;
    struct TraceHost;

    /// Validator-side adapter over the imported `expression-syntax` capability.
    #[derive(Debug)]
    struct ImportedSyntaxChecker {
        data_model: String,
    }

    impl ExpressionSyntaxChecker for ImportedSyntaxChecker {
        fn data_model(&self) -> &str {
            &self.data_model
        }

        fn check(&self, kind: ExpressionKind, source: &str) -> ExpressionCheck {
            let kind = match kind {
                ExpressionKind::Condition => wit_syntax::SyntaxKind::Condition,
                ExpressionKind::Value => wit_syntax::SyntaxKind::Value,
                ExpressionKind::Location => wit_syntax::SyntaxKind::Location,
                ExpressionKind::Script => wit_syntax::SyntaxKind::Script,
            };
            let Ok(issues) = wit_syntax::check_syntax(kind, source) else {
                return ExpressionCheck::NotChecked;
            };
            if issues.is_empty() {
                return ExpressionCheck::Valid;
            }
            let mut checked = Vec::with_capacity(issues.len().min(4));
            for issue in issues.into_iter().take(4) {
                let (Ok(start), Ok(end)) =
                    (usize::try_from(issue.start), usize::try_from(issue.end))
                else {
                    return ExpressionCheck::NotChecked;
                };
                // Provider offsets cross a trust boundary: out-of-range or
                // non-boundary offsets are declined, never trusted.
                if start > end
                    || end > source.len()
                    || !source.is_char_boundary(start)
                    || !source.is_char_boundary(end)
                    || issue.message.len() > 512
                {
                    return ExpressionCheck::NotChecked;
                }
                checked.push(SyntaxIssue {
                    start,
                    end,
                    message: issue.message,
                });
            }
            ExpressionCheck::Invalid(checked)
        }

        /// Forwards to the imported `check-effects`; the validator checks
        /// every returned span and bound before use (`report_effects`).
        fn check_effects(
            &self,
            fragments: &[EffectFragment<'_>],
            scripts: ExternalScripts,
        ) -> EffectCheck {
            let request: Vec<wit_syntax::EffectFragment> = fragments
                .iter()
                .map(|fragment| {
                    let (role, name) = match fragment.role {
                        EffectRole::Condition => (wit_syntax::EffectRole::Condition, None),
                        EffectRole::Log => (wit_syntax::EffectRole::Log, None),
                        EffectRole::Script => (wit_syntax::EffectRole::Script, None),
                        EffectRole::Data(id) => (wit_syntax::EffectRole::Data, Some(id.to_owned())),
                        EffectRole::Value => (wit_syntax::EffectRole::Value, None),
                        EffectRole::Location => (wit_syntax::EffectRole::Location, None),
                    };
                    wit_syntax::EffectFragment {
                        role,
                        name,
                        source: fragment.source.to_owned(),
                    }
                })
                .collect();
            let scripts = match scripts {
                ExternalScripts::Absent => wit_syntax::ExternalScripts::Absent,
                ExternalScripts::Present => wit_syntax::ExternalScripts::Present,
            };
            let Ok(findings) = wit_syntax::check_effects(&request, scripts) else {
                return EffectCheck::NotChecked;
            };
            let kind = |kind: wit_syntax::EffectKind| match kind {
                wit_syntax::EffectKind::Assignment => EffectKind::Assignment,
                wit_syntax::EffectKind::Update => EffectKind::Update,
                wit_syntax::EffectKind::Deletion => EffectKind::Deletion,
                wit_syntax::EffectKind::MutatingCall => EffectKind::MutatingCall,
                wit_syntax::EffectKind::Random => EffectKind::Random,
                wit_syntax::EffectKind::FunctionCall => EffectKind::FunctionCall,
                wit_syntax::EffectKind::UnknownCall => EffectKind::UnknownCall,
                wit_syntax::EffectKind::DynamicCode => EffectKind::DynamicCode,
                wit_syntax::EffectKind::ImplicitCall => EffectKind::ImplicitCall,
                wit_syntax::EffectKind::UnknownWrite => EffectKind::UnknownWrite,
                wit_syntax::EffectKind::DynamicScope => EffectKind::DynamicScope,
                wit_syntax::EffectKind::UnseenScript => EffectKind::UnseenScript,
                wit_syntax::EffectKind::TooComplex => EffectKind::TooComplex,
                wit_syntax::EffectKind::SystemVariableWrite => EffectKind::SystemVariableWrite,
            };
            let span = |span: wit_syntax::EffectSpan| -> Option<EffectSpan> {
                Some(EffectSpan {
                    fragment: usize::try_from(span.fragment).ok()?,
                    start: usize::try_from(span.start).ok()?,
                    end: usize::try_from(span.end).ok()?,
                })
            };
            let converted: Option<Vec<EffectFinding>> = findings
                .into_iter()
                .map(|finding| {
                    Some(EffectFinding {
                        certain: finding.certain,
                        kind: kind(finding.kind),
                        span: span(finding.span)?,
                        subject: finding.subject,
                        cause: match finding.cause {
                            Some(cause) => Some(EffectCause {
                                kind: kind(cause.kind),
                                span: span(cause.span)?,
                                subject: cause.subject,
                                function: cause.function,
                            }),
                            None => None,
                        },
                        operator: match finding.operator {
                            Some(operator) => Some(span(operator)?),
                            None => None,
                        },
                    })
                })
                .collect();
            converted.map_or(EffectCheck::NotChecked, EffectCheck::Checked)
        }
    }

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
                wit_telemetry::Module::Validator,
                wit_telemetry::Operation::Validate,
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
                    ErrorCategory::Report => wit_telemetry::ErrorCategory::Report,
                    _ => wit_telemetry::ErrorCategory::Validation,
                }),
            );
        }

        fn event(&self, span: &Self::Span, event: SignalEvent) {
            match event {
                SignalEvent::InputRejected => {
                    wit_telemetry::event(*span, wit_telemetry::SignalEvent::InputRejected);
                }
                SignalEvent::ArtifactProduced => {
                    wit_telemetry::event(*span, wit_telemetry::SignalEvent::ArtifactProduced);
                }
                _ => {}
            }
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
        #[allow(
            clippy::too_many_lines,
            reason = "boundary maps the complete request into one atomic report"
        )]
        fn validate(request: ValidationRequest) -> Result<Vec<u8>, ComponentError> {
            let parent = request.trace_context.as_ref().map(|context| TraceContext {
                traceparent: context.traceparent.clone(),
                tracestate: context.tracestate.clone(),
            });
            let span = WasmSpanAdapter::new(
                &TRACE_HOST,
                Module::ValidatorComponent,
                Operation::ValidatorComponentValidate,
                parent.as_ref(),
            );
            if let Err(error) = validate_request_boundary(&request) {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::InvalidInput);
                return Err(error);
            }
            let mut request = request;
            let Some(event_schemas) = scxml_event_schema::request_resources(
                std::mem::take(&mut request.event_schemas)
                    .into_iter()
                    .map(|resource| (resource.uri, resource.bytes)),
            ) else {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::InvalidInput);
                return Err(ComponentError {
                    version: ErrorVersion::V1,
                    code: ErrorCode::InvalidRequest,
                });
            };
            let result = (|| {
                let source_name = request.source_name.clone();
                let source_text = request.source.clone();
                let mut resolver = MemorySchemaResolver::new();
                for schema in &request.schemas {
                    resolver.insert(
                        schema.namespace_uri.clone(),
                        SchemaSource::new(schema.system_id.clone(), schema.bytes.clone()),
                    );
                }
                // The bundled official W3C SCXML 1.0 schema set governs the
                // core namespace unless the request supplies its own schema.
                let resolver = BundledSchemaResolver::new(resolver);
                let mut validator = Validator::new(resolver, XsdSchemaBackend)
                    .with_schema_mode(match request.policy {
                        SchemaPolicy::Strict => SchemaMode::Strict,
                        SchemaPolicy::Lax => SchemaMode::Lax,
                        SchemaPolicy::Off => SchemaMode::Off,
                    })
                    .with_extension_slots(
                        request
                            .packages
                            .iter()
                            .map(|package| {
                                (package.namespace_uri.clone(), package.env_slots.clone())
                            })
                            .collect(),
                    )
                    .with_event_schema_resources(event_schemas)
                    // The request carries no service packages, so no
                    // invoked-service type is bound: a literal one is
                    // `SCXML094`, exactly as `scxml-cli validate` reports
                    // it without `--service-package`.
                    .with_services(std::collections::BTreeMap::new());
                // The linked expression-syntax capability is explicit; a
                // provider that reports no data model disables checking.
                if let Some(data_model) = wit_syntax::supported_data_model()
                    && !data_model.is_empty()
                    && data_model.len() <= 64
                {
                    validator = validator.with_expression_checker(std::sync::Arc::new(
                        ImportedSyntaxChecker { data_model },
                    ));
                }
                let outcome = validator.validate_source(
                    DOCUMENT_SOURCE,
                    source_name.clone(),
                    source_text.clone(),
                );
                let schemas = request
                    .schemas
                    .iter()
                    .map(|schema| super::SchemaBinding {
                        namespace_uri: &schema.namespace_uri,
                        system_id: &schema.system_id,
                        bytes: &schema.bytes,
                    })
                    .collect::<Vec<_>>();
                let packages = request
                    .packages
                    .iter()
                    .map(|package| super::PackageBinding {
                        namespace_uri: &package.namespace_uri,
                        package_version: &package.package_version,
                        extension_artifact: &package.extension_artifact,
                        schema_artifact: &package.schema_artifact,
                        schema_sha256: &package.schema_sha256,
                        wit_world: &package.wit_world,
                        wit_version: &package.wit_version,
                        env_slots: &package.env_slots,
                        http_origins: &package.http_origins,
                    })
                    .collect::<Vec<_>>();
                let used_namespaces = outcome
                    .document
                    .as_ref()
                    .map_or_else(Vec::new, scxml_syntax::Document::used_namespaces);
                let mut diagnostics =
                    super::package_diagnostics(&schemas, &packages, &used_namespaces);
                if !diagnostics.is_empty() {
                    span.event(SignalEvent::InputRejected);
                    span.fail(ErrorCategory::Validation);
                }
                diagnostics.extend(
                    outcome
                        .report
                        .diagnostics()
                        .iter()
                        .map(to_canonical_diagnostic),
                );
                diagnostics.sort_by(|left, right| {
                    (
                        left.primary.span.start,
                        left.primary.span.end,
                        left.severity,
                        left.code.as_str(),
                        left.message.as_str(),
                    )
                        .cmp(&(
                            right.primary.span.start,
                            right.primary.span.end,
                            right.severity,
                            right.code.as_str(),
                            right.message.as_str(),
                        ))
                });
                let artifact = ArtifactEnvelope::validator(source_name, source_text, diagnostics)
                    .map_err(|_| canonical_report_error())?;
                let bytes = encode_canonical(&artifact).map_err(|_| canonical_report_error())?;
                span.event(SignalEvent::ArtifactProduced);
                Ok(bytes)
            })();
            if result.is_err() {
                span.fail(ErrorCategory::Validation);
            }
            result
        }
    }

    fn canonical_report_error() -> ComponentError {
        ComponentError {
            version: ErrorVersion::V1,
            code: ErrorCode::CanonicalReportFailed,
        }
    }

    /// Bounds every request field before validation; runs inside the
    /// component's entry span, which records the rejection.
    fn validate_request_boundary(request: &ValidationRequest) -> Result<(), ComponentError> {
        let mut boundary = ComponentRequestBoundary::new(
            &request.source_name,
            &request.source,
            request.schemas.len(),
            request.packages.len(),
        )
        .map_err(|_| ComponentError {
            version: ErrorVersion::V1,
            code: ErrorCode::InvalidRequest,
        })?;
        if let Some(context) = request.trace_context.as_ref() {
            boundary
                .add_optional_metadata("trace-context.traceparent", context.traceparent.as_deref())
                .map_err(|_| ComponentError {
                    version: ErrorVersion::V1,
                    code: ErrorCode::InvalidRequest,
                })?;
            boundary
                .add_optional_metadata("trace-context.tracestate", context.tracestate.as_deref())
                .map_err(|_| ComponentError {
                    version: ErrorVersion::V1,
                    code: ErrorCode::InvalidRequest,
                })?;
        }
        for schema in &request.schemas {
            boundary
                .add_schema(&schema.namespace_uri, &schema.system_id, &schema.bytes)
                .map_err(|_| ComponentError {
                    version: ErrorVersion::V1,
                    code: ErrorCode::InvalidRequest,
                })?;
        }
        let mut package_namespaces = BTreeSet::new();
        for package in &request.packages {
            for (field, value) in [
                ("package.namespace-uri", package.namespace_uri.as_str()),
                ("package.package-version", package.package_version.as_str()),
                (
                    "package.extension-artifact",
                    package.extension_artifact.as_str(),
                ),
                ("package.schema-artifact", package.schema_artifact.as_str()),
                ("package.schema-sha256", package.schema_sha256.as_str()),
                ("package.wit-world", package.wit_world.as_str()),
                ("package.wit-version", package.wit_version.as_str()),
            ] {
                boundary
                    .add_optional_metadata(field, Some(value))
                    .map_err(|_| ComponentError {
                        version: ErrorVersion::V1,
                        code: ErrorCode::InvalidRequest,
                    })?;
            }
            if !package_namespaces.insert(package.namespace_uri.as_str()) {
                return Err(ComponentError {
                    version: ErrorVersion::V1,
                    code: ErrorCode::InvalidRequest,
                });
            }
        }
        boundary.finish().map_err(|_| ComponentError {
            version: ErrorVersion::V1,
            code: ErrorCode::InvalidRequest,
        })
    }

    fn to_canonical_diagnostic(diagnostic: &scxml_validator_core::Diagnostic) -> Diagnostic {
        Diagnostic {
            severity: diagnostic.severity,
            code: diagnostic.code.clone(),
            category: diagnostic.category,
            message: diagnostic.message.clone(),
            primary: to_label(&diagnostic.primary),
            secondary: diagnostic.secondary.iter().map(to_label).collect(),
            notes: diagnostic.notes.clone(),
            help: diagnostic.help.clone(),
            spec_references: diagnostic.spec_references.clone(),
            fixes: diagnostic
                .fixes
                .iter()
                .map(|fix| Fix {
                    message: fix.message.clone(),
                    applicability: fix.applicability,
                    edits: fix
                        .edits
                        .iter()
                        .map(|edit| TextEdit {
                            span: ByteSpan {
                                start: edit.span.start as u64,
                                end: edit.span.end as u64,
                            },
                            replacement: edit.replacement.clone(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    fn to_label(label: &scxml_validator_core::Label) -> Label {
        Label {
            span: ByteSpan {
                start: label.span.start as u64,
                end: label.span.end as u64,
            },
            message: label.message.clone(),
        }
    }

    export!(Component);
}

/// Native marker for workspace tests.
#[must_use]
pub fn is_guest_component() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{PackageBinding, SchemaBinding, package_diagnostics};
    use scxml_reporting::{ArtifactEnvelope, decode_canonical, encode_canonical};
    use scxml_syntax::{SourceId, SourceText, parse_xml};
    use scxml_telemetry::{Recorder, install_recorder};
    use scxml_validator_core::{
        SCXML_NAMESPACE, SUPPORTED_NAMESPACE_WIT_VERSION, SUPPORTED_NAMESPACE_WIT_WORLD,
    };

    /// The source id of the documents these tests parse.
    const SOURCE: SourceId = SourceId::new(1);

    const EXTENSION_NAMESPACE: &str = "https://example.test/scxml/custom";
    const SCHEMA_SHA256: &str = "df0ad6e43880f09c90ebf95f19110178aba6890df0010ebda7485029e2b543b4";

    fn package(schema_sha256: &str) -> PackageBinding<'_> {
        PackageBinding {
            namespace_uri: EXTENSION_NAMESPACE,
            package_version: "1.0.0",
            extension_artifact: "custom.wasm",
            schema_artifact: "custom.xsd",
            schema_sha256,
            wit_world: SUPPORTED_NAMESPACE_WIT_WORLD,
            wit_version: SUPPORTED_NAMESPACE_WIT_VERSION,
            env_slots: &[],
            http_origins: &[],
        }
    }

    #[test]
    fn invalid_package_diagnostic_is_accepted_by_canonical_reporting() {
        let digest = "0".repeat(64);
        let packages = [package(&digest)];
        let diagnostics = package_diagnostics(&[], &packages, &[EXTENSION_NAMESPACE.to_owned()]);

        assert_eq!(diagnostics.len(), 1, "invalid package must emit PKG001");
        assert_eq!(diagnostics[0].code, "PKG001");
        assert!(
            diagnostics[0].fixes.is_empty(),
            "package metadata cannot be repaired with an honest source edit"
        );
        let artifact = ArtifactEnvelope::validator("test.scxml", "", diagnostics)
            .expect("PKG001 diagnostic must form a valid artifact");
        let bytes = encode_canonical(&artifact).expect("PKG001 artifact must encode canonically");
        assert_eq!(
            decode_canonical(&bytes).expect("PKG001 canonical bytes must decode"),
            artifact
        );
        assert_eq!(
            encode_canonical(&artifact).expect("repeated PKG001 encoding must succeed"),
            bytes,
            "identical invalid packages must emit byte-identical canonical artifacts"
        );
    }

    #[test]
    fn used_custom_schema_without_package_binding_fails_closed() {
        let schemas = [SchemaBinding {
            namespace_uri: EXTENSION_NAMESPACE,
            system_id: "custom.xsd",
            bytes: b"schema",
        }];
        let source = format!(
            r#"<scxml xmlns="{SCXML_NAMESPACE}" xmlns:custom="{EXTENSION_NAMESPACE}"><custom:action/></scxml>"#
        );
        let document = parse_xml(SourceText::new(SOURCE, "test.scxml", source))
            .expect("extension namespace fixture must parse");

        let diagnostics = package_diagnostics(&schemas, &[], &document.used_namespaces());

        assert_eq!(diagnostics.len(), 1, "missing binding must emit PKG001");
        assert_eq!(diagnostics[0].code, "PKG001");
    }

    #[test]
    fn core_scxml_schema_does_not_require_custom_package_binding() {
        let schemas = [SchemaBinding {
            namespace_uri: SCXML_NAMESPACE,
            system_id: "scxml.xsd",
            bytes: b"schema",
        }];

        assert!(
            package_diagnostics(&schemas, &[], &[SCXML_NAMESPACE.to_owned()]).is_empty(),
            "the core SCXML schema is not an extension extension package"
        );
    }

    #[test]
    fn exact_custom_schema_package_binding_is_accepted() {
        let schemas = [SchemaBinding {
            namespace_uri: EXTENSION_NAMESPACE,
            system_id: "custom.xsd",
            bytes: b"schema",
        }];
        let packages = [package(SCHEMA_SHA256)];

        assert!(
            package_diagnostics(&schemas, &packages, &[EXTENSION_NAMESPACE.to_owned()]).is_empty(),
            "exact URI, artifact, digest, and WIT identity must be accepted"
        );
    }

    #[test]
    fn mismatched_custom_schema_digest_fails_closed() {
        let schemas = [SchemaBinding {
            namespace_uri: EXTENSION_NAMESPACE,
            system_id: "custom.xsd",
            bytes: b"schema",
        }];
        let packages = [package(
            "0000000000000000000000000000000000000000000000000000000000000000",
        )];

        let diagnostics =
            package_diagnostics(&schemas, &packages, &[EXTENSION_NAMESPACE.to_owned()]);

        assert_eq!(diagnostics.len(), 1, "digest mismatch must emit PKG001");
        assert_eq!(diagnostics[0].code, "PKG001");
        assert!(diagnostics[0].fixes.is_empty());
    }

    #[test]
    fn mismatched_custom_package_wit_identity_fails_with_pkg001() {
        let schemas = [SchemaBinding {
            namespace_uri: EXTENSION_NAMESPACE,
            system_id: "custom.xsd",
            bytes: b"schema",
        }];
        let mut mismatched = package(SCHEMA_SHA256);
        mismatched.wit_version = "9.9.9";

        let diagnostics =
            package_diagnostics(&schemas, &[mismatched], &[EXTENSION_NAMESPACE.to_owned()]);

        assert_eq!(diagnostics.len(), 1, "WIT mismatch must emit PKG001");
        assert_eq!(diagnostics[0].code, "PKG001");
        assert!(diagnostics[0].fixes.is_empty());
    }

    #[test]
    fn used_unknown_extension_namespace_fails_closed() {
        let recorder = Recorder::default();
        let _guard = install_recorder(recorder.clone());
        let diagnostics = package_diagnostics(&[], &[], &[EXTENSION_NAMESPACE.to_owned()]);

        assert_eq!(
            diagnostics.len(),
            1,
            "unknown used extension namespace must emit PKG001"
        );
        assert_eq!(diagnostics[0].code, "PKG001");
        // Package verification runs inside the component's entry span and
        // records nothing of its own; the entry span records the rejection.
        assert!(recorder.observations().is_empty());
    }
}
