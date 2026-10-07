//! SCXML compiler WebAssembly guest.
//!
//! The guest emits portable Machine IR plus a deterministic package recipe
//! (`lowered-ir`), or packages that IR into a validated `component-wasm` by
//! composing the caller-supplied, pinned, prebuilt generic machine template
//! with a data-only `ir-source` provider (`scxml-packager`). It never
//! generates code, never reads the filesystem (template bytes arrive in the
//! request), never invokes a native compiler toolchain, and never produces a
//! standalone `core-wasm` module.

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
    use scxml_validator_core::{ExtensionPackageManifest, SchemaMode};

    wit_bindgen::generate!({
        world: "compiler",
        path: "../../crates/scxml-wit/wit/compiler",
    });

    use exports::stateforward::scxml_compiler::compiler_api::Guest;
    use stateforward::scxml_compiler::telemetry as wit_telemetry;
    use stateforward::scxml_compiler::types::{
        ArtifactKind, CompileRequest, CompileResult, ComponentError, ErrorCode, ErrorVersion,
        SchemaPolicy, ScriptResolution,
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
            let parent = parent.map(|context| wit_telemetry::TraceContext {
                traceparent: context.traceparent.clone(),
                tracestate: context.tracestate.clone(),
            });
            let (span, context) = wit_telemetry::start(
                wit_telemetry::Module::Compiler,
                wit_telemetry::Operation::Compile,
                parent.as_ref(),
            );
            (
                span,
                context.map(|context| TraceContext {
                    traceparent: context.traceparent,
                    tracestate: context.tracestate,
                }),
            )
        }

        fn end(&self, span: Self::Span, error: Option<ErrorCategory>) {
            wit_telemetry::end(
                span,
                error.map(|category| match category {
                    ErrorCategory::InvalidInput | ErrorCategory::Parse => {
                        wit_telemetry::ErrorCategory::InvalidInput
                    }
                    ErrorCategory::Validation => wit_telemetry::ErrorCategory::Validation,
                    ErrorCategory::Lowering => wit_telemetry::ErrorCategory::Lowering,
                    ErrorCategory::Packaging => wit_telemetry::ErrorCategory::Packaging,
                    _ => wit_telemetry::ErrorCategory::Report,
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

    impl Guest for Component {
        fn compile(request: CompileRequest) -> Result<CompileResult, ComponentError> {
            let parent = request.trace_context.as_ref().map(|context| TraceContext {
                traceparent: context.traceparent.clone(),
                tracestate: context.tracestate.clone(),
            });
            let span = WasmSpanAdapter::new(
                &TRACE_HOST,
                Module::CompilerComponent,
                Operation::CompilerComponentCompile,
                parent.as_ref(),
            );
            match super::compile(core_request(request)) {
                Ok(output) => {
                    span.event(SignalEvent::ArtifactProduced);
                    Ok(CompileResult {
                        report_artifact: output.report_artifact,
                        artifact_bytes: output.artifact_bytes,
                    })
                }
                Err(code) => {
                    span.event(SignalEvent::InputRejected);
                    span.fail(match code {
                        super::CompileErrorCode::InvalidRequest
                        | super::CompileErrorCode::UnsupportedArtifact => {
                            ErrorCategory::InvalidInput
                        }
                        super::CompileErrorCode::InvalidPackage
                        | super::CompileErrorCode::InvalidTemplate => ErrorCategory::Validation,
                        super::CompileErrorCode::CanonicalReportFailed => ErrorCategory::Report,
                        super::CompileErrorCode::PackagingFailed => ErrorCategory::Packaging,
                    });
                    Err(ComponentError {
                        version: ErrorVersion::V1,
                        code: match code {
                            super::CompileErrorCode::InvalidRequest => ErrorCode::InvalidRequest,
                            super::CompileErrorCode::InvalidPackage => ErrorCode::InvalidPackage,
                            super::CompileErrorCode::UnsupportedArtifact => {
                                ErrorCode::UnsupportedArtifact
                            }
                            super::CompileErrorCode::CanonicalReportFailed => {
                                ErrorCode::CanonicalReportFailed
                            }
                            super::CompileErrorCode::InvalidTemplate => ErrorCode::InvalidTemplate,
                            super::CompileErrorCode::PackagingFailed => ErrorCode::PackagingFailed,
                        },
                    })
                }
            }
        }
    }

    /// Converts the WIT request into the owned, WIT-independent form.
    fn core_request(request: CompileRequest) -> super::CompileRequest {
        let (traceparent, tracestate) = request.trace_context.map_or((None, None), |context| {
            (context.traceparent, context.tracestate)
        });
        super::CompileRequest {
            source_name: request.source_name,
            source: request.source,
            schemas: request
                .schemas
                .into_iter()
                .map(|schema| super::SchemaResource {
                    namespace_uri: schema.namespace_uri,
                    system_id: schema.system_id,
                    bytes: schema.bytes,
                })
                .collect(),
            packages: request
                .packages
                .into_iter()
                .map(|package| ExtensionPackageManifest {
                    namespace_uri: package.namespace_uri,
                    package_version: package.package_version,
                    extension_artifact: package.extension_artifact,
                    schema_artifact: package.schema_artifact,
                    schema_sha256: package.schema_sha256,
                    wit_world: package.wit_world,
                    wit_version: package.wit_version,
                    // The WIT package record carries no WASI permissions:
                    // compilation never links the extension component.
                    validator_artifact: None,
                    validator_sha256: None,
                })
                .collect(),
            schema_mode: match request.options.schema_policy {
                SchemaPolicy::Strict => SchemaMode::Strict,
                SchemaPolicy::Lax => SchemaMode::Lax,
                SchemaPolicy::Off => SchemaMode::Off,
            },
            requested_artifact: match request.options.requested_artifact {
                ArtifactKind::LoweredIr => super::RequestedArtifact::LoweredIr,
                ArtifactKind::CoreWasm => super::RequestedArtifact::CoreWasm,
                ArtifactKind::ComponentWasm => super::RequestedArtifact::ComponentWasm,
            },
            scripts: request
                .scripts
                .into_iter()
                .map(|script| scxml_compiler_core::ScriptPackage {
                    uri: script.uri,
                    media_type: script.media_type,
                    sha256: script.sha256,
                    bytes: script.bytes,
                })
                .collect(),
            script_resolution: match request.options.script_resolution {
                ScriptResolution::Compile => scxml_compiler_core::ScriptResolution::Compile,
                ScriptResolution::Runtime => scxml_compiler_core::ScriptResolution::Runtime,
            },
            event_schemas: request
                .event_schemas
                .into_iter()
                .map(|resource| (resource.uri, resource.bytes))
                .collect(),
            machine_template: request.machine_template.map(|template| {
                super::MachineTemplateInput {
                    bytes: template.bytes,
                    sha256: template.sha256,
                }
            }),
            traceparent,
            tracestate,
        }
    }

    export!(Component);
}

use scxml_compiler_core::{CompileInput, Compiler, NamespaceSchemaBinding};
use scxml_reporting::encode_canonical;
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_validator_core::{
    ComponentRequestBoundary, ExtensionPackageManifest, MemorySchemaResolver, PackageIdentity,
    SchemaMode, SchemaSource, Validator,
};
use scxml_validator_xsd::XsdSchemaBackend;

/// One XSD bound to one exact namespace URI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaResource {
    /// Exact namespace URI.
    pub namespace_uri: String,
    /// Schema system identifier.
    pub system_id: String,
    /// Schema bytes.
    pub bytes: Vec<u8>,
}

/// Owned, WIT-independent form of one `compile-request`.
#[derive(Clone, Debug)]
pub struct CompileRequest {
    /// Diagnostic source name.
    pub source_name: String,
    /// SCXML source text.
    pub source: String,
    /// Exact-namespace schema resources.
    pub schemas: Vec<SchemaResource>,
    /// extension package manifests.
    pub packages: Vec<ExtensionPackageManifest>,
    /// XSD validation policy.
    pub schema_mode: SchemaMode,
    /// Requested artifact phase.
    pub requested_artifact: RequestedArtifact,
    /// Script packages for `<script src>` URIs (compile-time resolution).
    pub scripts: Vec<scxml_compiler_core::ScriptPackage>,
    /// How `<script src>` references are resolved.
    pub script_resolution: scxml_compiler_core::ScriptResolution,
    /// The bytes of each schema file an `event:schema` references, keyed by
    /// the reference as authored (`event-schema-resource`).
    pub event_schemas: Vec<(String, Vec<u8>)>,
    /// Pinned generic machine template; required exactly for
    /// [`RequestedArtifact::ComponentWasm`].
    pub machine_template: Option<MachineTemplateInput>,
    /// Optional W3C `traceparent`, bounded as request metadata.
    pub traceparent: Option<String>,
    /// Optional W3C `tracestate`, bounded as request metadata.
    pub tracestate: Option<String>,
}

/// Requested artifact phase, mirroring the WIT compiler `artifact-kind`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestedArtifact {
    /// Portable Machine IR data.
    LoweredIr,
    /// A standalone core module; never produced.
    CoreWasm,
    /// The packaged machine component.
    ComponentWasm,
}

/// Owned pinned generic machine template from a `compile-request`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineTemplateInput {
    /// Exact template component bytes.
    pub bytes: Vec<u8>,
    /// Pinned lowercase hex SHA-256 of `bytes`.
    pub sha256: String,
}

/// Successful compilation output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileOutput {
    /// Canonical `scxml.report/v3` compiler report; the sole decision.
    pub report_artifact: Vec<u8>,
    /// Lowered IR or component bytes, present only for a successful report.
    pub artifact_bytes: Option<Vec<u8>>,
}

/// Closed compile failure, mirroring the WIT compiler `error-code`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileErrorCode {
    /// The request violated a documented bound or uniqueness rule.
    InvalidRequest,
    /// A package manifest failed its boundary checks.
    InvalidPackage,
    /// `core-wasm` was requested; it is never produced.
    UnsupportedArtifact,
    /// The canonical report could not be encoded.
    CanonicalReportFailed,
    /// The machine template failed its pinned identity or world checks.
    InvalidTemplate,
    /// Composing the IR provider with the template failed.
    PackagingFailed,
}

/// Validates the request boundary, then validates and lowers the source.
///
/// Invalid SCXML is not an error: it yields a report with diagnostics and no
/// artifact. Output is byte-deterministic for identical requests.
///
/// # Errors
///
/// Returns the closed [`CompileErrorCode`]; no report or artifact bytes are
/// produced on failure.
pub fn compile(mut request: CompileRequest) -> Result<CompileOutput, CompileErrorCode> {
    let span = observe(
        Module::CompilerComponent,
        Operation::CompilerComponentCompile,
    );
    let result = (|| {
        validate_request_boundary(&request)?;
        if request.requested_artifact == RequestedArtifact::CoreWasm {
            return Err(CompileErrorCode::UnsupportedArtifact);
        }
        let event_schemas =
            scxml_event_schema::request_resources(std::mem::take(&mut request.event_schemas))
                .ok_or(CompileErrorCode::InvalidRequest)?;
        let mut resolver = MemorySchemaResolver::new();
        for schema in &request.schemas {
            resolver.insert(
                schema.namespace_uri.clone(),
                SchemaSource::new(schema.system_id.clone(), schema.bytes.clone()),
            );
        }
        let package_schemas = request
            .schemas
            .iter()
            .map(|schema| NamespaceSchemaBinding {
                namespace_uri: schema.namespace_uri.clone(),
                schema: SchemaSource::new(schema.system_id.clone(), schema.bytes.clone()),
            })
            .collect();
        let compiler = Compiler::new(
            Validator::new(resolver, XsdSchemaBackend).with_schema_mode(request.schema_mode),
        );
        let wasm_scripts = request
            .scripts
            .iter()
            .filter(|script| script.media_type == scxml_script::WASM_MEDIA_TYPE)
            .cloned()
            .collect::<Vec<_>>();
        let mut output = compiler.compile(CompileInput {
            source_name: request.source_name,
            source: request.source,
            packages: request.packages,
            package_schemas,
            scripts: request.scripts,
            script_resolution: request.script_resolution,
            event_schemas,
        });
        // Package only an accepted lowering; rejected sources still yield a
        // report with diagnostics and no artifact.
        if let (Some(template), Some(lowered)) =
            (request.machine_template.as_ref(), output.artifact.as_ref())
        {
            let packaged = scxml_packager::package(
                lowered,
                scxml_packager::MachineTemplate {
                    bytes: &template.bytes,
                    sha256: &template.sha256,
                },
                &wasm_scripts
                    .iter()
                    .map(|script| scxml_packager::ScriptComponent {
                        uri: &script.uri,
                        bytes: &script.bytes,
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(package_error)?;
            output.artifact = Some(packaged);
        }
        let report_artifact = output
            .canonical_artifact()
            .and_then(|artifact| encode_canonical(&artifact))
            .map_err(|_| CompileErrorCode::CanonicalReportFailed)?;
        Ok(CompileOutput {
            report_artifact,
            artifact_bytes: output.artifact.map(|artifact| artifact.bytes),
        })
    })();
    match &result {
        Ok(_) => span.event(SignalEvent::ArtifactProduced),
        Err(code) => {
            span.event(SignalEvent::InputRejected);
            span.fail(match code {
                CompileErrorCode::InvalidRequest | CompileErrorCode::UnsupportedArtifact => {
                    ErrorCategory::InvalidInput
                }
                CompileErrorCode::InvalidPackage | CompileErrorCode::InvalidTemplate => {
                    ErrorCategory::Validation
                }
                CompileErrorCode::CanonicalReportFailed => ErrorCategory::Report,
                CompileErrorCode::PackagingFailed => ErrorCategory::Packaging,
            });
        }
    }
    result
}

fn package_error(error: scxml_packager::PackageError) -> CompileErrorCode {
    match error {
        scxml_packager::PackageError::TemplateSize
        | scxml_packager::PackageError::InvalidTemplateDigest
        | scxml_packager::PackageError::TemplateDigestMismatch
        | scxml_packager::PackageError::InvalidTemplate
        | scxml_packager::PackageError::TemplateWorldMismatch => CompileErrorCode::InvalidTemplate,
        scxml_packager::PackageError::NotLoweredIr
        | scxml_packager::PackageError::InvalidIr(_)
        | scxml_packager::PackageError::NonCanonicalIr
        | scxml_packager::PackageError::InconsistentRecipe
        | scxml_packager::PackageError::Composition(_)
        | scxml_packager::PackageError::OutputTooLarge
        | scxml_packager::PackageError::MissingScript
        | scxml_packager::PackageError::UnexpectedScript
        | scxml_packager::PackageError::InvalidScript(_) => CompileErrorCode::PackagingFailed,
    }
}

fn validate_request_boundary(request: &CompileRequest) -> Result<(), CompileErrorCode> {
    let mut boundary = ComponentRequestBoundary::new(
        &request.source_name,
        &request.source,
        request.schemas.len(),
        request.packages.len(),
    )
    .map_err(|_| CompileErrorCode::InvalidRequest)?;
    boundary
        .add_optional_metadata("trace-context.traceparent", request.traceparent.as_deref())
        .map_err(|_| CompileErrorCode::InvalidRequest)?;
    boundary
        .add_optional_metadata("trace-context.tracestate", request.tracestate.as_deref())
        .map_err(|_| CompileErrorCode::InvalidRequest)?;
    for schema in &request.schemas {
        boundary
            .add_schema(&schema.namespace_uri, &schema.system_id, &schema.bytes)
            .map_err(|_| CompileErrorCode::InvalidRequest)?;
    }
    for package in &request.packages {
        boundary
            .add_package(&PackageIdentity {
                namespace_uri: &package.namespace_uri,
                package_version: &package.package_version,
                extension_artifact: &package.extension_artifact,
                schema_artifact: &package.schema_artifact,
                schema_sha256: &package.schema_sha256,
                wit_world: &package.wit_world,
                wit_version: &package.wit_version,
            })
            .map_err(|_| CompileErrorCode::InvalidPackage)?;
    }
    validate_script_packages(&request.scripts)?;
    match (
        request.requested_artifact,
        request.machine_template.as_ref(),
    ) {
        (RequestedArtifact::ComponentWasm, Some(template)) => {
            let digest_syntax = template.sha256.len() == 64
                && template
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
            if template.bytes.len() > scxml_packager::MAX_TEMPLATE_BYTES || !digest_syntax {
                return Err(CompileErrorCode::InvalidRequest);
            }
        }
        (RequestedArtifact::ComponentWasm, None)
        | (RequestedArtifact::LoweredIr | RequestedArtifact::CoreWasm, Some(_)) => {
            return Err(CompileErrorCode::InvalidRequest);
        }
        (RequestedArtifact::LoweredIr | RequestedArtifact::CoreWasm, None) => {}
    }
    boundary
        .finish()
        .map_err(|_| CompileErrorCode::InvalidRequest)
}

/// Maximum UTF-8 bytes of one script package media type.
pub const MAX_SCRIPT_MEDIA_TYPE_BYTES: usize = 4096;

/// Request-boundary bounds for script packages (checked before parsing).
fn validate_script_packages(
    scripts: &[scxml_compiler_core::ScriptPackage],
) -> Result<(), CompileErrorCode> {
    let mut total = 0_usize;
    if scripts.len() > scxml_script::MAX_SCRIPT_PACKAGES
        || scripts.iter().any(|script| {
            total = total.saturating_add(script.bytes.len());
            script.uri.is_empty()
                || script.uri.len() > scxml_script::MAX_SCRIPT_URI_BYTES
                || script.media_type.is_empty()
                || script.media_type.len() > MAX_SCRIPT_MEDIA_TYPE_BYTES
                || script.sha256.len() > 64
                || script.bytes.len() > scxml_script::MAX_SCRIPT_COMPONENT_BYTES
        })
        || total > scxml_script::MAX_TOTAL_SCRIPT_BYTES
    {
        Err(CompileErrorCode::InvalidRequest)
    } else {
        Ok(())
    }
}

/// Native marker for workspace tests.
#[must_use]
pub fn is_guest_component() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use scxml_reporting::{ArtifactPayload, CompilerStatus, decode_canonical};

    const VALID: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="ready"><state id="ready"/></scxml>"#;

    fn request(source: &str) -> CompileRequest {
        CompileRequest {
            source_name: "machine.scxml".into(),
            source: source.into(),
            schemas: Vec::new(),
            packages: Vec::new(),
            schema_mode: SchemaMode::Off,
            requested_artifact: RequestedArtifact::LoweredIr,
            scripts: Vec::new(),
            script_resolution: scxml_compiler_core::ScriptResolution::Compile,
            event_schemas: Vec::new(),
            machine_template: None,
            traceparent: None,
            tracestate: None,
        }
    }

    fn compiler_status(report: &[u8]) -> CompilerStatus {
        let envelope = decode_canonical(report).expect("compiler report must be canonical");
        match envelope.payload {
            ArtifactPayload::Compiler { compiler } => compiler.status,
            _ => panic!("compiler must emit a compiler-kind report"),
        }
    }

    #[test]
    fn accepted_source_emits_canonical_report_and_lowered_ir_deterministically() {
        let first = compile(request(VALID)).expect("valid request compiles");
        let second = compile(request(VALID)).expect("valid request compiles again");
        assert_eq!(first, second, "compiler output must be byte-deterministic");
        assert_eq!(
            compiler_status(&first.report_artifact),
            CompilerStatus::Emitted
        );
        let ir = first
            .artifact_bytes
            .expect("a successful report carries the lowered IR bytes");
        let ir: serde_json::Value = serde_json::from_slice(&ir).expect("lowered IR is JSON data");
        assert_eq!(
            ir["version"],
            serde_json::json!(scxml_compiler_core::MACHINE_IR_VERSION)
        );
    }

    #[test]
    fn invalid_scxml_is_a_report_not_a_component_error() {
        let output = compile(request("<scxml")).expect("malformed SCXML still yields a report");
        assert_eq!(
            compiler_status(&output.report_artifact),
            CompilerStatus::Rejected
        );
        assert!(output.artifact_bytes.is_none());
    }

    #[test]
    fn boundary_violations_fail_closed_with_closed_codes() {
        let mut unsupported = request(VALID);
        unsupported.requested_artifact = RequestedArtifact::CoreWasm;
        assert_eq!(
            compile(unsupported),
            Err(CompileErrorCode::UnsupportedArtifact)
        );

        let mut missing_template = request(VALID);
        missing_template.requested_artifact = RequestedArtifact::ComponentWasm;
        assert_eq!(
            compile(missing_template),
            Err(CompileErrorCode::InvalidRequest)
        );

        let template = MachineTemplateInput {
            bytes: b"not a component".to_vec(),
            sha256: "0".repeat(64),
        };
        let mut stray_template = request(VALID);
        stray_template.machine_template = Some(template.clone());
        assert_eq!(
            compile(stray_template),
            Err(CompileErrorCode::InvalidRequest),
            "a template is rejected unless component-wasm is requested"
        );

        let mut malformed_digest = request(VALID);
        malformed_digest.requested_artifact = RequestedArtifact::ComponentWasm;
        malformed_digest.machine_template = Some(MachineTemplateInput {
            sha256: "ABC".into(),
            ..template.clone()
        });
        assert_eq!(
            compile(malformed_digest),
            Err(CompileErrorCode::InvalidRequest)
        );

        let mut oversized_template = request(VALID);
        oversized_template.requested_artifact = RequestedArtifact::ComponentWasm;
        oversized_template.machine_template = Some(MachineTemplateInput {
            bytes: vec![0; scxml_packager::MAX_TEMPLATE_BYTES + 1],
            ..template.clone()
        });
        assert_eq!(
            compile(oversized_template),
            Err(CompileErrorCode::InvalidRequest)
        );

        let mut mismatched = request(VALID);
        mismatched.requested_artifact = RequestedArtifact::ComponentWasm;
        mismatched.machine_template = Some(template.clone());
        assert_eq!(
            compile(mismatched),
            Err(CompileErrorCode::InvalidTemplate),
            "a template that does not match its pin is rejected"
        );

        let mut rejected_source = request("<scxml");
        rejected_source.requested_artifact = RequestedArtifact::ComponentWasm;
        rejected_source.machine_template = Some(template);
        let output = compile(rejected_source).expect("invalid SCXML still yields a report");
        assert_eq!(
            compiler_status(&output.report_artifact),
            CompilerStatus::Rejected,
            "invalid SCXML is reported before any packaging"
        );
        assert!(output.artifact_bytes.is_none());

        let mut unnamed = request(VALID);
        unnamed.source_name = String::new();
        assert_eq!(compile(unnamed), Err(CompileErrorCode::InvalidRequest));

        let oversized = request(&" ".repeat(scxml_validator_core::MAX_COMPONENT_SOURCE_BYTES + 1));
        assert_eq!(compile(oversized), Err(CompileErrorCode::InvalidRequest));

        let mut too_many = request(VALID);
        too_many.schemas = (0..=scxml_validator_core::MAX_COMPONENT_SCHEMA_COUNT)
            .map(|index| SchemaResource {
                namespace_uri: format!("urn:schema:{index}"),
                system_id: "schema.xsd".into(),
                bytes: Vec::new(),
            })
            .collect();
        assert_eq!(compile(too_many), Err(CompileErrorCode::InvalidRequest));

        let mut foreign_world = request(VALID);
        foreign_world.packages = vec![ExtensionPackageManifest {
            namespace_uri: "urn:example".into(),
            package_version: "1.0.0".into(),
            extension_artifact: "example.wasm".into(),
            schema_artifact: "example.xsd".into(),
            schema_sha256: "0".repeat(64),
            wit_world: "stateforward:other/world".into(),
            wit_version: scxml_validator_core::SUPPORTED_NAMESPACE_WIT_VERSION.into(),
            validator_artifact: None,
            validator_sha256: None,
        }];
        assert_eq!(
            compile(foreign_world),
            Err(CompileErrorCode::InvalidPackage)
        );
    }
}
