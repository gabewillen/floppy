//! Portable SCXML validation, complete deterministic lowering, and artifact planning.
//!
//! Machine IR is deterministic portable data, not WebAssembly. Component
//! packaging is a distinct later phase owned by the `scxml-packager` crate,
//! which composes the IR with a pinned, prebuilt generic machine template;
//! this crate only records the recipe both phases share.
//!
//! # Example
//!
//! Compile SCXML to canonical Machine IR (schema validation off here; a real
//! caller supplies an XSD backend) and decode the untrusted IR bytes again:
//!
//! ```
//! use scxml_compiler_core::{
//!     ArtifactKind, CompileInput, Compiler, MACHINE_IR_VERSION, MachineIr, ScriptResolution,
//! };
//! use scxml_syntax::Document;
//! use scxml_validator_core::{
//!     MemorySchemaResolver, Report, SchemaBackendError, SchemaMode, SchemaSource,
//!     SchemaValidator, Validator,
//! };
//!
//! #[derive(Debug)]
//! struct NoSchemaBackend;
//!
//! impl SchemaValidator for NoSchemaBackend {
//!     fn validate(
//!         &self,
//!         _document: &Document,
//!         _schemas: &[SchemaSource],
//!     ) -> Result<Report, SchemaBackendError> {
//!         Ok(Report::new())
//!     }
//! }
//!
//! let compiler = Compiler::new(
//!     Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
//!         .with_schema_mode(SchemaMode::Off),
//! );
//! let output = compiler.compile(CompileInput {
//!     source_name: "light.scxml".into(),
//!     source: r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="off">
//!   <state id="off"><transition event="toggle" target="on"/></state>
//!   <state id="on"><transition event="toggle" target="off"/></state>
//! </scxml>"#
//!         .into(),
//!     packages: Vec::new(),
//!     package_schemas: Vec::new(),
//!     scripts: Vec::new(),
//!     script_resolution: ScriptResolution::Compile,
//!     event_schemas: std::collections::BTreeMap::new(),
//! });
//! assert!(!output.validation.report.has_errors());
//! let artifact = output.artifact.expect("valid source lowers to an artifact");
//! assert_eq!(artifact.kind, ArtifactKind::LoweredIr);
//!
//! // The IR bytes are canonical: decoding validates them, and re-encoding
//! // reproduces the exact bytes.
//! let machine = MachineIr::from_json(&artifact.bytes)?;
//! assert_eq!(machine.version, MACHINE_IR_VERSION);
//! assert_eq!(machine.to_json()?, artifact.bytes);
//! assert!(MachineIr::from_json(b"{}").is_err());
//! # Ok::<(), scxml_compiler_core::LowerError>(())
//! ```

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use scxml_validator_core::{
    Diagnostic, DiagnosticCategory, ExtensionPackageManifest, MAX_COMPONENT_METADATA_BYTES,
    MAX_COMPONENT_PACKAGE_COUNT, MAX_COMPONENT_REQUEST_BYTES, MAX_COMPONENT_SCHEMA_BYTES,
    MAX_COMPONENT_SCHEMA_COUNT, MAX_COMPONENT_TOTAL_METADATA_BYTES,
    MAX_COMPONENT_TOTAL_SCHEMA_BYTES, Report, SchemaResolver, SchemaSource, SchemaValidator,
    ValidationOutcome, Validator,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod ir;
mod lower;

pub use ir::*;
pub use lower::{lower, lower_with_event_schemas};

/// SCXML namespace used during lowering.
pub const SCXML_NAMESPACE: &str = "http://www.w3.org/2005/07/scxml";
/// Built-in XML namespace, which does not require an extension package.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// Stable Machine IR format version, the `ir-source` interface that delivers
/// it to the generic machine, the template identity, and the phase media
/// types, owned by the canonical report contract so producer and reporters
/// share one source of truth.
pub use scxml_reporting::{
    COMPONENT_WASM_FILE_EXTENSION, COMPONENT_WASM_MEDIA_TYPE, LOWERED_IR_FILE_EXTENSION,
    LOWERED_IR_MEDIA_TYPE, MACHINE_IR_SOURCE_INTERFACE, MACHINE_IR_VERSION,
    MACHINE_TEMPLATE_VERSION, MACHINE_TEMPLATE_WORLD,
};

/// The source id of the one document a compilation reads: every span points into it.
const DOCUMENT_SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);

const MAX_SAFE_VALUE_CHARS: usize = 128;

/// Artifact phase. Callers must not treat phases as interchangeable.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    /// Portable deterministic machine data, not WebAssembly.
    LoweredIr,
    /// Core WebAssembly module bytes.
    CoreWasm,
    /// Final WebAssembly Component bytes.
    ComponentWasm,
}

/// Deterministic recipe shared by the `lowered-ir` and `component-wasm` phases.
///
/// Invariants (enforced again by the canonical report decoder): for
/// `lowered-ir`, `ir_sha256` is the artifact digest, `template_sha256` is
/// absent, and `next_phase` is `component-wasm`; for `component-wasm`,
/// `ir_sha256` names the embedded IR, `template_sha256` names the exact
/// pinned generic machine template the IR was composed with, and
/// `next_phase` is absent because the phase is terminal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRecipe {
    /// Prebuilt generic machine template world.
    pub template_world: String,
    /// Template version selected by the compiler.
    pub template_version: String,
    /// SHA-256 of the exact template bytes a component was composed with.
    pub template_sha256: Option<String>,
    /// SHA-256 of the exact IR bytes.
    pub ir_sha256: String,
    /// WIT interface through which the machine receives the IR bytes.
    pub ir_source_interface: String,
    /// Required extension component artifact identifiers.
    pub linked_extension_components: Vec<String>,
    /// The next artifact phase, absent for the terminal `component-wasm` phase.
    pub next_phase: Option<ArtifactKind>,
}

/// Honest compiler artifact of one phase.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedArtifact {
    /// Phase of `bytes` (`LoweredIr` from [`Compiler::compile`];
    /// `ComponentWasm` only from the packager).
    pub kind: ArtifactKind,
    /// MIME type for the bytes.
    pub media_type: String,
    /// Recommended file extension.
    pub file_extension: String,
    /// Canonical Machine IR bytes or validated component bytes, per `kind`.
    pub bytes: Vec<u8>,
    /// Deterministic package recipe.
    pub recipe: PackageRecipe,
}

/// Compiler input independent of filesystem or network access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileInput {
    /// Display name used by diagnostics.
    pub source_name: String,
    /// UTF-8 SCXML source.
    pub source: String,
    /// Declarative extension package manifests.
    pub packages: Vec<ExtensionPackageManifest>,
    /// Caller-supplied schemas used to verify extension package integrity.
    pub package_schemas: Vec<NamespaceSchemaBinding>,
    /// Script packages for `<script src>` URIs (compile-time resolution).
    pub scripts: Vec<ScriptPackage>,
    /// How `<script src>` references are resolved.
    pub script_resolution: ScriptResolution,
    /// The bytes of each schema file the document's `event:schema`
    /// attributes reference, keyed by the reference's resource part as
    /// authored (the value before `#`); the caller reads them relative to
    /// the document. Validation reports a missing one (`SCXML088`), and
    /// lowering embeds each resolved schema in the Machine IR.
    pub event_schemas: scxml_event_schema::Resources,
}

/// How the compiler treats `<script src>` references (SCXML 1.0 section 5.8).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScriptResolution {
    /// Every `src` must be satisfied by exactly one supplied
    /// [`ScriptPackage`]; the compiler authenticates it and pins its media
    /// type, SHA-256, and (for Wasm) world into the Machine IR. A missing or
    /// invalid package is a `COMP003` error on the `src` attribute.
    #[default]
    Compile,
    /// References stay unpinned and the host's runtime `script-loader`
    /// resolves them at document load (dynamic or agent-authored SCXML;
    /// the only mode browsers use). Supplying packages is an error.
    Runtime,
}

/// Exact bytes for one `<script src>` URI, supplied by the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptPackage {
    /// The `src` URI exactly as authored (byte comparison).
    pub uri: String,
    /// Media type: `application/wasm` or a script language of the data model.
    pub media_type: String,
    /// Expected lowercase hex SHA-256 of `bytes`.
    pub sha256: String,
    /// Exact script bytes.
    pub bytes: Vec<u8>,
}

/// Prefix of the package-recipe entry that pins one Wasm script component.
pub const SCRIPT_COMPONENT_RECIPE_PREFIX: &str = "script-sha256:";

/// Exact namespace-to-schema binding available to package verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamespaceSchemaBinding {
    /// Exact namespace URI associated with the schema bytes.
    pub namespace_uri: String,
    /// Schema artifact identity and bytes.
    pub schema: SchemaSource,
}

/// Validation result plus optional artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileOutput {
    /// Diagnostics from XML, XSD, package, semantic, and lowering phases.
    pub validation: ValidationOutcome,
    /// Present only when there are no errors and lowering succeeded.
    pub artifact: Option<PlannedArtifact>,
}

impl CompileOutput {
    /// Converts the complete compiler decision into the canonical report envelope.
    ///
    /// Reporters receive producer-selected status and output metadata; they do
    /// not rerun validation, lowering, hashing, or artifact selection. This reads
    /// only the immutable output and returns a newly owned envelope.
    ///
    /// # Errors
    ///
    /// Returns an invariant or serialization error from the canonical contract.
    pub fn canonical_artifact(
        &self,
    ) -> Result<scxml_reporting::ArtifactEnvelope, scxml_reporting::ArtifactError> {
        let span = observe(Module::CompilerCore, Operation::CompilerCanonicalArtifact);
        let result = (|| {
            let validator = self
                .validation
                .report
                .canonical_artifact(&self.validation.source)?;
            let Some(validator) = validator.validator_payload() else {
                return Err(scxml_reporting::ArtifactError::InconsistentCompilerStatus);
            };
            let diagnostics = validator.diagnostics.clone();
            let (status, output) = match self.artifact.as_ref() {
                None => (scxml_reporting::CompilerStatus::Rejected, None),
                Some(artifact) => {
                    let package_recipe = serde_json::to_value(&artifact.recipe)
                        .and_then(|recipe| serde_json::to_vec(&recipe))
                        .map_err(|error| {
                            scxml_reporting::ArtifactError::InvalidJson(error.to_string())
                        })?;
                    let byte_length = u64::try_from(artifact.bytes.len())
                        .map_err(|_| scxml_reporting::ArtifactError::InconsistentCompilerStatus)?;
                    (
                        scxml_reporting::CompilerStatus::Emitted,
                        Some(scxml_reporting::CompilerOutput {
                            artifact_kind: match artifact.kind {
                                ArtifactKind::LoweredIr => "lowered-ir",
                                ArtifactKind::CoreWasm => "core-wasm",
                                ArtifactKind::ComponentWasm => "component-wasm",
                            }
                            .into(),
                            media_type: artifact.media_type.clone(),
                            file_extension: artifact.file_extension.clone(),
                            byte_length,
                            sha256: hex::encode(Sha256::digest(&artifact.bytes)),
                            package_recipe,
                        }),
                    )
                }
            };
            scxml_reporting::ArtifactEnvelope::compiler(
                self.validation.source.name(),
                self.validation.source.text(),
                diagnostics,
                status,
                output,
            )
        })();
        if result.is_err() {
            span.fail(ErrorCategory::Report);
        } else {
            span.event(SignalEvent::ArtifactProduced);
        }
        result
    }
}

/// Pure compiler parameterized by schema resolver and validator backend.
#[derive(Debug)]
pub struct Compiler<R, B> {
    validator: Validator<R, B>,
}

impl<R, B> Compiler<R, B> {
    /// Creates a compiler over the supplied validator pipeline.
    ///
    /// Ownership of the validator is transferred to the compiler. Construction
    /// has no side effects and cannot fail.
    #[must_use]
    pub const fn new(validator: Validator<R, B>) -> Self {
        Self { validator }
    }
}

impl<R: SchemaResolver, B: SchemaValidator> Compiler<R, B> {
    /// Validates and lowers source to portable IR and a component package recipe.
    ///
    /// The input is consumed. No filesystem, network, process, clock, random,
    /// or toolchain operation is performed. Validation/lowering failure commits
    /// no artifact and is returned as canonical diagnostics. Every used extension
    /// namespace requires exactly one supplied manifest and matching schema;
    /// every supplied manifest is verified even when its namespace is unused.
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "one ordered pipeline: validate, verify packages, pin scripts, lower, record the recipe"
    )]
    pub fn compile(&self, input: CompileInput) -> CompileOutput {
        let span = observe(Module::CompilerCore, Operation::CompilerCompile);
        let validation = self.validator.validate_source_with_event_schemas(
            DOCUMENT_SOURCE,
            input.source_name,
            input.source,
            &input.event_schemas,
        );
        let Some(document) = validation.document.as_ref() else {
            span.fail(ErrorCategory::Validation);
            return CompileOutput {
                validation,
                artifact: None,
            };
        };
        let root_span = document.root.start_tag_span;
        let linked_extension_components = match verify_extension_packages(
            document,
            &input.packages,
            &input.package_schemas,
        ) {
            Ok(components) => components,
            Err(failure) => {
                span.event(SignalEvent::InputRejected);
                span.fail(ErrorCategory::Validation);
                let diagnostic = match failure {
                    PackageFailure::Invalid(message) => {
                        Diagnostic::error("COMP002", message, root_span)
                            .with_category(DiagnosticCategory::Package)
                            .with_help(
                                "supply one exact verified extension package and schema binding for every used extension namespace",
                            )
                    }
                    PackageFailure::Unbound(namespace) => {
                        unbound_namespace_diagnostic(document, &namespace)
                    }
                };
                let mut validation = validation;
                validation.report.push(diagnostic);
                validation.report.sort();
                return CompileOutput {
                    validation,
                    artifact: None,
                };
            }
        };
        if validation.report.has_errors() {
            span.fail(ErrorCategory::Validation);
            return CompileOutput {
                validation,
                artifact: None,
            };
        }
        let script_pins =
            match resolve_script_packages(document, &input.scripts, input.script_resolution) {
                Ok(pins) => pins,
                Err(diagnostics) => {
                    span.event(SignalEvent::InputRejected);
                    span.fail(ErrorCategory::Validation);
                    let mut validation = validation;
                    for diagnostic in diagnostics {
                        validation.report.push(diagnostic);
                    }
                    validation.report.sort();
                    return CompileOutput {
                        validation,
                        artifact: None,
                    };
                }
            };
        let mut linked_extension_components = linked_extension_components;
        linked_extension_components.extend(
            script_pins
                .values()
                .filter(|pin| pin.pinned.media_type == scxml_script::WASM_MEDIA_TYPE)
                .map(|pin| format!("{SCRIPT_COMPONENT_RECIPE_PREFIX}{}", pin.pinned.sha256)),
        );
        linked_extension_components.sort();
        linked_extension_components.dedup();
        match lower::lower_located(document, &input.event_schemas)
            .and_then(|ir| apply_script_pins(ir, &script_pins).map_err(|error| (error, None)))
            .and_then(|ir| ir.to_json().map_err(|error| (error, None)))
        {
            Ok(bytes) => {
                let recipe = PackageRecipe {
                    template_world: MACHINE_TEMPLATE_WORLD.into(),
                    template_version: MACHINE_TEMPLATE_VERSION.into(),
                    template_sha256: None,
                    ir_sha256: hex::encode(Sha256::digest(&bytes)),
                    ir_source_interface: MACHINE_IR_SOURCE_INTERFACE.into(),
                    linked_extension_components,
                    next_phase: Some(ArtifactKind::ComponentWasm),
                };
                span.event(SignalEvent::ArtifactProduced);
                CompileOutput {
                    validation,
                    artifact: Some(PlannedArtifact {
                        kind: ArtifactKind::LoweredIr,
                        media_type: LOWERED_IR_MEDIA_TYPE.into(),
                        file_extension: LOWERED_IR_FILE_EXTENSION.into(),
                        bytes,
                        recipe,
                    }),
                }
            }
            Err((error, location)) => {
                span.fail(ErrorCategory::Lowering);
                let diagnostic = lowering_diagnostic(document, &error, location);
                let mut validation = validation;
                validation.report.push(diagnostic);
                validation.report.sort();
                CompileOutput {
                    validation,
                    artifact: None,
                }
            }
        }
    }
}

/// `COMP002` for a used extension namespace with no extension package: at the
/// first element (or attribute) in the namespace, with its declaration as a
/// secondary label. Executable-content extensions execute only through a bound
/// extension component, so the document fails closed instead of losing them.
fn unbound_namespace_diagnostic(document: &scxml_syntax::Document, namespace: &str) -> Diagnostic {
    let usage = scxml_validator_core::spans::first_namespace_use(document, namespace);
    let declaration = scxml_validator_core::spans::namespace_declaration_value(document, namespace);
    let primary = usage
        .or(declaration)
        .unwrap_or(document.root.start_tag_span);
    let mut diagnostic = Diagnostic::error(
        "COMP002",
        format!(
            "extension namespace `{}` has no extension package",
            safe_value(namespace)
        ),
        primary,
    )
    .with_category(DiagnosticCategory::Package)
    .with_primary_message("no extension component is bound to execute this")
    .with_note("extension-namespace elements execute only in the namespace's extension component; compiling without a verified package binding would silently drop them")
    .with_help(
        "bind the namespace with its package manifest (`scx compile --extension-package MANIFEST.json`), or remove its elements",
    )
    .with_spec_reference("SCXML 1.0 §4.10 Extensibility of Executable Content");
    if let (Some(usage), Some(declaration)) = (usage, declaration)
        && usage != declaration
    {
        diagnostic = diagnostic.with_secondary(
            scxml_validator_core::Label::new(declaration).with_message("namespace declared here"),
        );
    }
    diagnostic
}

/// `COMP001` for a validated document that lowering rejected: at the name
/// of the responsible element (or the offending text) when lowering located
/// it, otherwise at the root element's name. The validator reports every
/// structural rejection lowering makes, so this is reached only for a
/// Machine IR bound that exists after lowering (the encoded IR size) or a
/// validator gap.
fn lowering_diagnostic(
    document: &scxml_syntax::Document,
    error: &LowerError,
    location: Option<scxml_syntax::Span>,
) -> Diagnostic {
    let span = location.unwrap_or_else(|| {
        scxml_validator_core::spans::element_name_span(document, &document.root)
    });
    Diagnostic::error(
        "COMP001",
        format!("cannot lower this document: {error}"),
        span,
    )
    .with_category(DiagnosticCategory::Internal)
    .with_primary_message(if location.is_some() {
        "lowering failed here"
    } else {
        "lowering failed for the whole machine"
    })
    .with_note("the compiler lowers only constructs the engine implements, within fixed Machine IR bounds, and fails closed instead of dropping anything; `validate` reports every structural rejection first, so this finding means a Machine IR bound only lowering can measure (the encoded IR size) or a construct the validator does not yet report")
    .with_help(match error {
        LowerError::LimitExceeded { .. } => {
            "reduce the machine below the named bound, for example by splitting it into machines it invokes"
        }
        _ => {
            "change or remove the construct named in the message; a document that validates cleanly but fails here is a validator gap worth reporting"
        }
    })
    .with_spec_reference("SCXML 1.0 §3.2 <scxml>")
}

/// One authenticated script package, keyed by URI.
#[derive(Clone, Debug)]
struct ScriptPin {
    pinned: PinnedScriptIr,
    /// Data-model-language text embedded in the IR; `None` for Wasm.
    text: Option<String>,
}

const SPEC_SCRIPT: &str = "SCXML 1.0 §5.8 <script>";
const SCRIPT_REJECTION_NOTE: &str = "SCXML 1.0 §5.8: if the script specified by `src` cannot be downloaded or parsed at document load time, the document is rejected";

fn script_diagnostic(message: String, span: scxml_syntax::Span) -> Diagnostic {
    Diagnostic::error("COMP003", message, span)
        .with_category(DiagnosticCategory::Package)
        .with_note(SCRIPT_REJECTION_NOTE)
        .with_spec_reference(SPEC_SCRIPT)
}

/// Authenticates caller-supplied script packages against every
/// `<script src>` in the document and returns the pins keyed by URI.
///
/// Compile mode: each `src` needs exactly one package for its exact URI;
/// each package must verify (bounds, media type for the document's data
/// model, SHA-256, Wasm component world) and be referenced. Runtime mode:
/// packages are rejected and no pin is produced. Every failure is a
/// `COMP003` error on the exact `src` value span (or the root start tag for
/// request-level failures).
fn resolve_script_packages(
    document: &scxml_syntax::Document,
    packages: &[ScriptPackage],
    resolution: ScriptResolution,
) -> Result<BTreeMap<String, ScriptPin>, Vec<Diagnostic>> {
    let root = document.root.start_tag_span;
    let mut sites = Vec::new();
    document.root.visit(&mut |element| {
        if element.name.is(Some(SCXML_NAMESPACE), "script")
            && let Some(src) = element.attribute("src")
        {
            sites.push((
                scxml_syntax::collapse_whitespace(&src.value).into_owned(),
                src.value_span,
            ));
        }
    });
    if let Some(diagnostic) = script_request_diagnostic(packages, resolution, root) {
        return Err(vec![diagnostic]);
    }
    if resolution == ScriptResolution::Runtime {
        return Ok(BTreeMap::new());
    }
    let data_model = Some(document.root.attribute("datamodel").map_or(
        std::borrow::Cow::Borrowed(scxml_validator_core::DEFAULT_DATA_MODEL),
        |attribute| scxml_syntax::collapse_whitespace(&attribute.value),
    ));
    let mut by_uri = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for package in packages {
        if by_uri.insert(package.uri.as_str(), package).is_some() {
            diagnostics.push(
                script_diagnostic(
                    format!(
                        "more than one script package supplies `{}`",
                        safe_value(&package.uri)
                    ),
                    root,
                )
                .with_primary_message("duplicate script package URI")
                .with_help("supply exactly one script package per `src` URI"),
            );
        }
    }
    let mut pins = BTreeMap::new();
    for (uri, value_span) in &sites {
        match pin_script_site(
            uri,
            *value_span,
            by_uri.get(uri.as_str()).copied(),
            data_model.as_deref(),
        ) {
            Ok(pin) => {
                pins.insert(uri.clone(), pin);
            }
            Err(diagnostic) => diagnostics.push(*diagnostic),
        }
    }
    for package in packages {
        if !sites.iter().any(|(uri, _)| uri == &package.uri) {
            diagnostics.push(
                script_diagnostic(
                    format!(
                        "script package `{}` is not referenced by any `<script src>`",
                        safe_value(&package.uri)
                    ),
                    root,
                )
                .with_primary_message("unused script package")
                .with_help("remove the package, or reference its exact URI from a `<script src>`"),
            );
        }
    }
    if diagnostics.is_empty() {
        Ok(pins)
    } else {
        Err(diagnostics)
    }
}

/// Request-level script failures, reported on the root start tag: packages
/// under runtime resolution, or packages over the count or byte bound.
fn script_request_diagnostic(
    packages: &[ScriptPackage],
    resolution: ScriptResolution,
    root: scxml_syntax::Span,
) -> Option<Diagnostic> {
    if resolution == ScriptResolution::Runtime && !packages.is_empty() {
        Some(
            script_diagnostic(
                "script packages were supplied, but script resolution is `runtime`".into(),
                root,
            )
            .with_primary_message("runtime resolution takes no script packages")
            .with_help(
                "drop the script packages, or compile with `compile` script resolution to pin them",
            ),
        )
    } else {
        let total = packages.iter().try_fold(0_usize, |total, package| {
            total.checked_add(package.bytes.len())
        });
        (packages.len() > scxml_script::MAX_SCRIPT_PACKAGES
            || total.is_none_or(|total| total > scxml_script::MAX_TOTAL_SCRIPT_BYTES))
        .then(|| {
            script_diagnostic("script packages exceed the compiler bounds".into(), root)
                .with_primary_message("too many or too large script packages")
                .with_help(format!(
                    "supply at most {} script packages and {} bytes together",
                    scxml_script::MAX_SCRIPT_PACKAGES,
                    scxml_script::MAX_TOTAL_SCRIPT_BYTES
                ))
        })
    }
}

/// Authenticates the package for one `src` site and returns its pin, or a
/// `COMP003` error on the exact `src` value span.
fn pin_script_site(
    uri: &str,
    value_span: scxml_syntax::Span,
    package: Option<&ScriptPackage>,
    data_model: Option<&str>,
) -> Result<ScriptPin, Box<Diagnostic>> {
    let Some(package) = package else {
        return Err(Box::new(script_diagnostic(
            format!("no script package supplies `src` URI `{}`", safe_value(uri)),
            value_span,
        )
        .with_primary_message("unresolved script source")
        .with_help("supply a script package for this exact URI (bytes, media type, and SHA-256), or compile with `runtime` script resolution so the host's script loader resolves it at load")));
    };
    let verified = scxml_script::verify_script(
        uri,
        &package.media_type,
        &package.bytes,
        data_model,
        Some(&package.sha256),
    )
    .map_err(|error| {
        Box::new(
            script_diagnostic(
                format!(
                    "script package for `{}` is rejected: {}",
                    safe_value(uri),
                    error.detail()
                ),
                value_span,
            )
            .with_primary_message(format!("rejected script package ({})", error.reason()))
            .with_help(script_error_help(error)),
        )
    })?;
    let is_component = verified.kind == scxml_script::ScriptKind::WasmComponent;
    Ok(ScriptPin {
        pinned: PinnedScriptIr {
            media_type: package.media_type.clone(),
            sha256: verified.sha256,
            wit_world: is_component.then(|| scxml_script::SCRIPT_WORLD.to_owned()),
        },
        text: if is_component {
            None
        } else {
            String::from_utf8(package.bytes.clone()).ok()
        },
    })
}

fn script_error_help(error: scxml_script::ScriptError) -> String {
    match error {
        scxml_script::ScriptError::InvalidUri => {
            "use a non-empty URI without control characters".into()
        }
        scxml_script::ScriptError::UnsupportedMediaType => format!(
            "declare `{}` for a Wasm script component, or a script language of the document's data model (ECMAScript: {})",
            scxml_script::WASM_MEDIA_TYPE,
            scxml_script::ECMASCRIPT_MEDIA_TYPES.join(", ")
        ),
        scxml_script::ScriptError::TooLarge if cfg!(target_arch = "wasm32") => format!(
            "keep Wasm script components within {} bytes and script text within {} bytes",
            scxml_script::MAX_SCRIPT_COMPONENT_BYTES,
            scxml_script::MAX_SCRIPT_TEXT_BYTES
        ),
        scxml_script::ScriptError::TooLarge => format!(
            "keep Wasm script components within {} bytes and script text non-empty",
            scxml_script::MAX_SCRIPT_COMPONENT_BYTES
        ),
        scxml_script::ScriptError::InvalidDigest | scxml_script::ScriptError::DigestMismatch => {
            "pin the lowercase hex SHA-256 of the exact package bytes".into()
        }
        scxml_script::ScriptError::NotAComponent => {
            "build the script as a Component Model component (a core module or other bytes cannot run)".into()
        }
        scxml_script::ScriptError::WorldMismatch => format!(
            "implement `{}`: export only `{}` and import only {}",
            scxml_script::SCRIPT_WORLD,
            scxml_script::SCRIPT_API_INTERFACE,
            scxml_script::SCRIPT_ALLOWED_IMPORTS.join(", ")
        ),
        scxml_script::ScriptError::InvalidText => "encode script text as UTF-8".into(),
    }
}

/// Pins every lowered `src` reference from `pins`; references without a
/// pin stay unpinned (runtime resolution).
fn apply_script_pins(
    mut machine: MachineIr,
    pins: &BTreeMap<String, ScriptPin>,
) -> Result<MachineIr, LowerError> {
    machine
        .try_update_script_sources(&mut |source, body| {
            if let Some(pin) = pins.get(&source.uri) {
                if !scxml_syntax::is_xml_blank(body) {
                    return Err(LowerError::InvalidStructure(
                        "script cannot combine src and inline body",
                    ));
                }
                source.pinned = Some(pin.pinned.clone());
                *body = pin.text.clone().unwrap_or_default();
            }
            Ok(())
        })
        .and_then(|()| machine.validate())
        .map(|()| machine)
}

/// Why extension package binding rejected a document.
enum PackageFailure {
    /// A supplied package or schema binding is invalid (static message).
    Invalid(String),
    /// A used extension namespace (the exact URI) has no package.
    Unbound(String),
}

impl From<String> for PackageFailure {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}

fn verify_extension_packages(
    document: &scxml_syntax::Document,
    packages: &[ExtensionPackageManifest],
    schemas: &[NamespaceSchemaBinding],
) -> Result<Vec<String>, PackageFailure> {
    if packages.len() > MAX_COMPONENT_PACKAGE_COUNT {
        return Err("extension package count exceeds the compiler limit"
            .to_owned()
            .into());
    }
    let (schemas_by_namespace, total_schema_bytes, mut total_metadata_bytes) =
        index_package_schemas(schemas)?;

    let mut packages_by_namespace = BTreeMap::new();
    for package in packages {
        for value in [
            package.namespace_uri.as_str(),
            package.package_version.as_str(),
            package.extension_artifact.as_str(),
            package.schema_artifact.as_str(),
            package.schema_sha256.as_str(),
            package.wit_world.as_str(),
            package.wit_version.as_str(),
        ] {
            total_metadata_bytes =
                total_metadata_bytes
                    .checked_add(value.len())
                    .ok_or_else(|| {
                        PackageFailure::from(
                            "extension package metadata aggregate exceeds the compiler limit"
                                .to_owned(),
                        )
                    })?;
        }
        if total_metadata_bytes > MAX_COMPONENT_TOTAL_METADATA_BYTES {
            return Err(
                "extension package metadata aggregate exceeds the compiler limit"
                    .to_owned()
                    .into(),
            );
        }
        let schema = schemas_by_namespace
            .get(package.namespace_uri.as_str())
            .ok_or_else(|| {
                PackageFailure::from(
                    "extension package requires exactly one exact schema binding".to_owned(),
                )
            })?;
        package.verify_schema(schema)?;
        if packages_by_namespace
            .insert(package.namespace_uri.as_str(), package)
            .is_some()
        {
            return Err(
                "extension namespace requires exactly one package manifest binding"
                    .to_owned()
                    .into(),
            );
        }
    }
    let total_request_bytes = document
        .source
        .text()
        .len()
        .checked_add(total_schema_bytes)
        .and_then(|total| total.checked_add(total_metadata_bytes))
        .ok_or_else(|| {
            PackageFailure::from("extension package request exceeds the compiler limit".to_owned())
        })?;
    if total_request_bytes > MAX_COMPONENT_REQUEST_BYTES {
        return Err("extension package request exceeds the compiler limit"
            .to_owned()
            .into());
    }

    let used_extension_namespaces = document
        .used_namespaces()
        .into_iter()
        .filter(|namespace| {
            namespace != SCXML_NAMESPACE
                && namespace != XML_NAMESPACE
                && namespace != scxml_types::ENV_NAMESPACE_URI
                && namespace != scxml_types::EVENT_NAMESPACE_URI
        })
        .collect::<BTreeSet<_>>();
    let mut linked_extension_components = used_extension_namespaces
        .iter()
        .map(|namespace| {
            packages_by_namespace
                .get(namespace.as_str())
                .map(|package| package.extension_artifact.clone())
                .ok_or_else(|| PackageFailure::Unbound(namespace.clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    linked_extension_components.sort();
    if linked_extension_components
        .windows(2)
        // `windows(2)` always yields slices of exactly length 2 (the
        // call is unreachable for input shorter than two elements), so
        // `pair[0]`/`pair[1]` cannot panic: the slice length invariant
        // is guaranteed by the iterator's own contract.
        .any(|pair| pair[0] == pair[1])
    {
        return Err(
            "used extension namespaces must bind distinct extension artifacts"
                .to_owned()
                .into(),
        );
    }
    Ok(linked_extension_components)
}

fn index_package_schemas(
    schemas: &[NamespaceSchemaBinding],
) -> Result<(BTreeMap<&str, &SchemaSource>, usize, usize), String> {
    if schemas.len() > MAX_COMPONENT_SCHEMA_COUNT {
        return Err("extension package schema count exceeds the compiler limit".to_owned());
    }
    let mut schemas_by_namespace = BTreeMap::new();
    let mut total_schema_bytes = 0_usize;
    let mut total_metadata_bytes = 0_usize;
    for binding in schemas {
        if binding.namespace_uri.is_empty()
            || binding.namespace_uri.len() > MAX_COMPONENT_METADATA_BYTES
        {
            return Err("extension package schema URI is empty or oversized".to_owned());
        }
        if binding.schema.system_id.is_empty()
            || binding.schema.system_id.len() > MAX_COMPONENT_METADATA_BYTES
        {
            return Err("extension package schema system-id is empty or oversized".to_owned());
        }
        if binding.schema.bytes.len() > MAX_COMPONENT_SCHEMA_BYTES {
            return Err("extension package schema exceeds the compiler limit".to_owned());
        }
        total_schema_bytes = total_schema_bytes
            .checked_add(binding.schema.bytes.len())
            .ok_or_else(|| {
                "extension package schema aggregate exceeds the compiler limit".to_owned()
            })?;
        if total_schema_bytes > MAX_COMPONENT_TOTAL_SCHEMA_BYTES {
            return Err("extension package schema aggregate exceeds the compiler limit".to_owned());
        }
        total_metadata_bytes = total_metadata_bytes
            .checked_add(binding.namespace_uri.len())
            .and_then(|total| total.checked_add(binding.schema.system_id.len()))
            .ok_or_else(|| {
                "extension package metadata aggregate exceeds the compiler limit".to_owned()
            })?;
        if schemas_by_namespace
            .insert(binding.namespace_uri.as_str(), &binding.schema)
            .is_some()
        {
            return Err("extension package schema URI must be unique".to_owned());
        }
    }
    if total_metadata_bytes > MAX_COMPONENT_TOTAL_METADATA_BYTES {
        return Err("extension package metadata aggregate exceeds the compiler limit".to_owned());
    }
    Ok((
        schemas_by_namespace,
        total_schema_bytes,
        total_metadata_bytes,
    ))
}

/// Lowering, encoding, or IR-decoding failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LowerError {
    /// Source has no executable states.
    NoStates,
    /// A required source attribute is absent or empty.
    MissingAttribute {
        /// Bounded element name.
        element: String,
        /// Static attribute name.
        attribute: &'static str,
    },
    /// A source element is not represented by this schema.
    UnsupportedElement(String),
    /// A source attribute is not represented by this schema.
    UnsupportedAttribute {
        /// Bounded element name.
        element: String,
        /// Bounded attribute name.
        attribute: String,
    },
    /// An enumerated source attribute has an unsupported value.
    InvalidAttributeValue {
        /// Static element name.
        element: &'static str,
        /// Static attribute name.
        attribute: &'static str,
        /// Bounded rejected value.
        value: String,
    },
    /// State identifiers must be globally unique.
    DuplicateStateId(String),
    /// State table exceeded the portable index width.
    TooManyStates,
    /// Transition table exceeded the portable index width.
    TooManyTransitions,
    /// Source document-order ordinals exceeded the portable width.
    TooManySourceElements,
    /// Initial identifier does not resolve.
    UnknownInitial(String),
    /// An initial/history transition requires a target.
    TransitionWithoutTarget,
    /// Transition target does not resolve.
    UnknownTarget(String),
    /// Untrusted JSON did not match the closed schema.
    InvalidIrSchema,
    /// IR version is unsupported.
    UnsupportedIrVersion(u32),
    /// Valid IR could not be serialized.
    IrSerialization,
    /// A declared deterministic resource bound was exceeded.
    LimitExceeded {
        /// Static field/category name.
        field: &'static str,
        /// Maximum accepted count or byte length.
        limit: usize,
    },
    /// A table index or ownership reference is invalid.
    InvalidReference(&'static str),
    /// A closed structural invariant is invalid.
    InvalidStructure(&'static str),
    /// An `event:schema` does not resolve (bounded message); the validator
    /// reports each cause first (`SCXML085`-`SCXML092`).
    InvalidEventSchema(String),
    /// A field the engine turns into an event name, send ID, or invoke ID
    /// (or a `done.state.<ID>` or `done.invoke.<ID>` event it raises) is
    /// not a name: empty, containing white space, or over
    /// [`scxml_types::MAX_NAME_BYTES`] bytes.
    InvalidName(&'static str),
}

impl fmt::Display for LowerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoStates => formatter.write_str("SCXML document has no executable states"),
            Self::MissingAttribute { element, attribute } => {
                write!(
                    formatter,
                    "element `{element}` requires attribute `{attribute}`"
                )
            }
            Self::UnsupportedElement(element) => {
                write!(formatter, "unsupported SCXML element `{element}`")
            }
            Self::UnsupportedAttribute { element, attribute } => write!(
                formatter,
                "unsupported attribute `{attribute}` on element `{element}`"
            ),
            Self::InvalidAttributeValue {
                element,
                attribute,
                value,
            } => write!(
                formatter,
                "invalid value `{value}` for `{attribute}` on `{element}`"
            ),
            Self::DuplicateStateId(id) => write!(formatter, "duplicate state id `{id}`"),
            Self::TooManyStates => formatter.write_str("machine has more than u32::MAX states"),
            Self::TooManyTransitions => {
                formatter.write_str("machine has more than u32::MAX transitions")
            }
            Self::TooManySourceElements => {
                formatter.write_str("source has more than u32::MAX ordered elements")
            }
            Self::UnknownInitial(id) => write!(formatter, "unknown initial state `{id}`"),
            Self::TransitionWithoutTarget => {
                formatter.write_str("initial or history transition requires a target")
            }
            Self::UnknownTarget(id) => write!(formatter, "unknown transition target `{id}`"),
            Self::InvalidIrSchema => {
                formatter.write_str("Machine IR JSON violates the closed schema")
            }
            Self::UnsupportedIrVersion(version) => {
                write!(formatter, "unsupported Machine IR version {version}")
            }
            Self::IrSerialization => formatter.write_str("Machine IR serialization failed"),
            Self::LimitExceeded { field, limit } => {
                write!(formatter, "{field} exceeds the limit of {limit}")
            }
            Self::InvalidReference(field) => write!(formatter, "invalid IR reference: {field}"),
            Self::InvalidStructure(rule) => write!(formatter, "invalid SCXML/IR structure: {rule}"),
            Self::InvalidEventSchema(message) => {
                write!(formatter, "invalid event:schema: {message}")
            }
            Self::InvalidName(field) => write!(
                formatter,
                "invalid IR name: {field} is empty, contains white space, or exceeds {} bytes",
                scxml_types::MAX_NAME_BYTES
            ),
        }
    }
}

impl std::error::Error for LowerError {}

/// Convenience for compiler guests/CLIs that need only diagnostics on failure.
///
/// The report is borrowed and not mutated.
#[must_use]
pub fn has_errors(report: &Report) -> bool {
    report.has_errors()
}

fn safe_value(value: &str) -> String {
    value.chars().take(MAX_SAFE_VALUE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pinned `<script src>` whose body is not XML white space (here
    /// U+1680, which Unicode but not XML counts as white space) combines
    /// `src` with inline content (SCXML 1.0 §5.8), so pinning refuses it.
    #[test]
    fn pinning_refuses_a_body_of_other_unicode_white_space() {
        const URI: &str = "lib.js";
        const TEXT: &str = "x = 1;";
        const SOURCE: scxml_syntax::SourceId = scxml_syntax::SourceId::new(1);
        let pinned = |inline: &str| {
            let document = scxml_syntax::parse_xml(scxml_syntax::SourceText::new(
                SOURCE,
                "case.scxml",
                r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript"><script src="lib.js"/><state id="s"/></scxml>"#,
            ))
            .expect("well-formed");
            let mut machine = lower::lower(&document).expect("lowers");
            machine
                .try_update_script_sources(&mut |_, body| {
                    inline.clone_into(body);
                    Ok::<(), LowerError>(())
                })
                .expect("the body is set");
            let pins = BTreeMap::from([(
                URI.to_owned(),
                ScriptPin {
                    pinned: PinnedScriptIr {
                        media_type: "application/ecmascript".to_owned(),
                        sha256: scxml_script::sha256_hex(TEXT.as_bytes()),
                        wit_world: None,
                    },
                    text: Some(TEXT.to_owned()),
                },
            )]);
            apply_script_pins(machine, &pins).map(|_| ())
        };
        assert_eq!(pinned(" \t"), Ok(()), "XML white space is no body");
        assert_eq!(
            pinned("\u{1680}"),
            Err(LowerError::InvalidStructure(
                "script cannot combine src and inline body"
            ))
        );
    }
}
