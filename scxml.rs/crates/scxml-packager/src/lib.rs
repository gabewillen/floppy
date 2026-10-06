//! Codegen-free `component-wasm` packaging of lowered Machine IR.
//!
//! The generic machine component (`stateforward:scxml/machine`) contains the
//! deterministic engine and imports its immutable Machine IR through
//! `stateforward:scxml/ir-source`. Packaging never generates or compiles code:
//!
//! 1. an internal encoder emits a tiny data-only component exporting
//!    `ir-source`, whose core module holds the IR bytes in one active data
//!    segment of a non-growable memory and returns a constant pointer to them;
//! 2. [`package`] composes that provider with the caller-supplied, pinned,
//!    prebuilt generic machine template so the result imports only the
//!    machine's capability interfaces and exports `machine-api`;
//! 3. when the IR pins Wasm script components (`<script src>`, SCXML 1.0
//!    section 5.8), an internal encoder emits a second data-only
//!    component holding exactly those authenticated component bytes, and
//!    the package additionally exports its `script-bundle` interface so an
//!    ahead-of-time host serves pinned scripts from the artifact itself.
//!    Unpinned references stay with the host's runtime `script-loader`.
//!
//! Everything here is pure Rust (`wasm-encoder`, `wasmparser`, `wac-graph`),
//! performs no filesystem, network, process, clock, or randomness access, and
//! compiles for `wasm32-wasip2`, so the compiler component can package inside
//! Wasm. Callers acquire template bytes and their pinned SHA-256 identity
//! through their own capability boundary.
//!
//! [`package`] is the only public packaging entry point: it decodes and
//! validates the IR (bounds, version, schema, semantics, canonical bytes, and
//! recipe), authenticates every script against its pin, and validates the
//! template before any bytes are encoded or composed, so no unvalidated IR
//! can reach a `component-wasm` artifact.
//!
//! Browser hosts never use packaged components: they ship the generic
//! machine once and feed IR as data through `ir-source` (runtime mode).
//! Packaging serves non-browser ahead-of-time hosts only.
//!
//! Telemetry: the two public boundaries, [`package`] and [`validate_template`],
//! each record one span through `scxml-telemetry` with the static `packager`
//! module label and a closed operation label; IR validation, provider and
//! bundle encoding, and composition run inside the [`package`] span. Failures
//! carry only the closed `packaging`/`invalid-input`/`validation` categories.
//! No digest, name, byte count, or payload is ever a telemetry attribute.
//!
//! # Example
//!
//! [`package`] checks every precondition before it encodes anything: here a
//! lowered artifact whose IR bytes were tampered with is rejected, and so is
//! a template whose bytes do not match their pinned digest.
//!
//! ```
//! use scxml_packager::{IrRejection, MachineTemplate, PackageError, package, validate_template};
//! # use scxml_compiler_core::{CompileInput, Compiler, ScriptResolution};
//! # use scxml_syntax::Document;
//! # use scxml_validator_core::{
//! #     MemorySchemaResolver, Report, SchemaBackendError, SchemaMode, SchemaSource,
//! #     SchemaValidator, Validator,
//! # };
//! # #[derive(Debug)]
//! # struct NoSchemaBackend;
//! # impl SchemaValidator for NoSchemaBackend {
//! #     fn validate(&self, _: &Document, _: &[SchemaSource]) -> Result<Report, SchemaBackendError> {
//! #         Ok(Report::new())
//! #     }
//! # }
//! # let compiler = Compiler::new(
//! #     Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
//! #         .with_schema_mode(SchemaMode::Off),
//! # );
//! # let lowered = compiler
//! #     .compile(CompileInput {
//! #         source_name: "m.scxml".into(),
//! #         source: r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0"><final id="done"/></scxml>"#.into(),
//! #         packages: Vec::new(),
//! #         package_schemas: Vec::new(),
//! #         scripts: Vec::new(),
//! #         script_resolution: ScriptResolution::Compile,
//! #         event_schemas: std::collections::BTreeMap::new(),
//! #     })
//! #     .artifact
//! #     .expect("fixture lowers");
//! // `lowered` is the `lowered-ir` artifact produced by `scxml-compiler-core`.
//! let template_bytes = b"\0asm\x0d\0\x01\0".as_slice();
//! let template = MachineTemplate {
//!     bytes: template_bytes,
//!     sha256: "0000000000000000000000000000000000000000000000000000000000000000",
//! };
//! assert_eq!(validate_template(template), Err(PackageError::TemplateDigestMismatch));
//!
//! let mut tampered = lowered.clone();
//! tampered.bytes.truncate(tampered.bytes.len() - 1);
//! assert_eq!(
//!     package(&tampered, template, &[]),
//!     Err(PackageError::InvalidIr(IrRejection::Schema))
//! );
//! ```

#![forbid(unsafe_code)]

use std::fmt;

use scxml_compiler_core::{
    ArtifactKind, COMPONENT_WASM_FILE_EXTENSION, COMPONENT_WASM_MEDIA_TYPE, LowerError,
    MACHINE_IR_SOURCE_INTERFACE, MACHINE_TEMPLATE_VERSION, MACHINE_TEMPLATE_WORLD, MAX_IR_BYTES,
    MachineIr, PackageRecipe, PlannedArtifact,
};
use scxml_telemetry::{ErrorCategory, Module, Operation, SignalEvent, observe};
use sha2::{Digest as _, Sha256};

/// Maximum accepted generic machine template size in bytes (8 MiB).
pub const MAX_TEMPLATE_BYTES: usize = 8 * 1024 * 1024;

/// Maximum emitted packaged component size in bytes: the template, the IR,
/// the bundled script components, and a fixed 1 MiB allowance for the
/// providers and composition envelope.
pub const MAX_PACKAGED_COMPONENT_BYTES: usize = MAX_TEMPLATE_BYTES
    .saturating_add(MAX_IR_BYTES)
    .saturating_add(scxml_script::MAX_TOTAL_SCRIPT_BYTES)
    .saturating_add(1024 * 1024);

/// Exact export set of a package that bundles script components, sorted.
pub const SCRIPTED_PACKAGE_EXPORTS: [&str; 2] =
    [MACHINE_API_INTERFACE, scxml_script::SCRIPT_BUNDLE_INTERFACE];

/// The machine export every template must provide and every package exports.
pub const MACHINE_API_INTERFACE: &str = "stateforward:scxml/machine-api@0.2.0";

/// Exact import set of the pinned generic machine template, sorted.
pub const TEMPLATE_IMPORTS: [&str; 4] = [
    "stateforward:scxml/ir-source@0.2.0",
    "stateforward:scxml/machine-capability-types@0.2.0",
    "stateforward:scxml/telemetry@0.2.0",
    "stateforward:scxml/types@0.2.0",
];

/// Byte offset of the IR payload inside the provider's linear memory; the
/// eight bytes before it are the canonical-ABI return area `(ptr, len)`.
const IR_OFFSET: u32 = 8;
/// Core Wasm page size.
const PAGE_BYTES: u64 = 65_536;
/// Provider core export returning the return-area pointer.
const PROVIDER_FUNCTION: &str = "machine-ir";
/// Provider core memory export.
const PROVIDER_MEMORY: &str = "memory";
/// Script bundle core export returning the return-area pointer.
const BUNDLE_FUNCTION: &str = "scripts";
/// Canonical-ABI size of one `tuple<string, string, list<u8>>` element.
const BUNDLE_ELEMENT_BYTES: u32 = 24;

/// Exact bytes of one Wasm script component pinned by the Machine IR,
/// supplied by the caller for bundling (for example the same script
/// package the compiler pinned).
#[derive(Clone, Copy, Debug)]
pub struct ScriptComponent<'a> {
    /// The `src` URI exactly as authored.
    pub uri: &'a str,
    /// Exact component bytes; authenticated against the IR pin.
    pub bytes: &'a [u8],
}

/// A caller-supplied, pinned generic machine template.
///
/// `bytes` are untrusted until [`validate_template`] accepts them: they must
/// be at most [`MAX_TEMPLATE_BYTES`], hash to `sha256` (64 lowercase hex
/// characters that the caller pinned out of band, for example from the
/// capability script's artifact manifest), validate as a component, import
/// exactly [`TEMPLATE_IMPORTS`], and export exactly [`MACHINE_API_INTERFACE`].
#[derive(Clone, Copy, Debug)]
pub struct MachineTemplate<'a> {
    /// Exact template component bytes.
    pub bytes: &'a [u8],
    /// Pinned lowercase hex SHA-256 of `bytes`.
    pub sha256: &'a str,
}

/// Closed packaging failure. Messages are static and never echo input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PackageError {
    /// The input artifact is not a `lowered-ir` artifact.
    NotLoweredIr,
    /// The IR failed bounds, version, schema, or semantic validation; the
    /// closed, input-free [`IrRejection`] names the failing check.
    InvalidIr(IrRejection),
    /// The IR bytes are not the canonical encoding of the IR they decode to.
    NonCanonicalIr,
    /// The input recipe does not describe the supplied IR bytes.
    InconsistentRecipe,
    /// The template is empty or exceeds [`MAX_TEMPLATE_BYTES`].
    TemplateSize,
    /// The pinned template digest is not 64 lowercase hex characters.
    InvalidTemplateDigest,
    /// The template bytes do not hash to the pinned digest.
    TemplateDigestMismatch,
    /// The template is not a valid WebAssembly component.
    InvalidTemplate,
    /// The template's imports or exports differ from the pinned machine world.
    TemplateWorldMismatch,
    /// Encoding a provider or composing the graph failed at the named stage.
    Composition(CompositionStage),
    /// The composed component exceeds [`MAX_PACKAGED_COMPONENT_BYTES`].
    OutputTooLarge,
    /// A Wasm script component pinned by the IR was not supplied.
    MissingScript,
    /// A supplied script is not pinned by the IR as a Wasm script component,
    /// or its URI is duplicated.
    UnexpectedScript,
    /// A supplied script failed authentication against its IR pin; the
    /// closed, payload-free [`scxml_script::ScriptError`] names the failing
    /// check (bounds, digest, component encoding, or script world).
    InvalidScript(scxml_script::ScriptError),
}

/// Closed cause of [`PackageError::InvalidIr`]. Carries no input text; the
/// only payload is the rejected Machine IR version number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IrRejection {
    /// The IR exceeds [`MAX_IR_BYTES`].
    TooLarge,
    /// The IR declares a Machine IR version other than
    /// [`scxml_compiler_core::MACHINE_IR_VERSION`] (for example IR produced
    /// by another release); carries the declared version.
    UnsupportedVersion(u32),
    /// The IR is not JSON matching the closed Machine IR schema.
    Schema,
    /// The IR decodes but breaks a Machine IR invariant: a declared bound,
    /// a table reference, or the state-graph structure.
    Semantic,
    /// The decoded IR has no canonical encoding.
    Unencodable,
}

impl IrRejection {
    /// Closed cause of one Machine IR decode failure.
    fn from_lower(error: &LowerError) -> Self {
        match error {
            LowerError::LimitExceeded {
                field: "IR bytes", ..
            } => Self::TooLarge,
            LowerError::UnsupportedIrVersion(version) => Self::UnsupportedVersion(*version),
            LowerError::InvalidIrSchema => Self::Schema,
            LowerError::IrSerialization => Self::Unencodable,
            _ => Self::Semantic,
        }
    }
}

impl fmt::Display for IrRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("it exceeds the Machine IR byte bound"),
            Self::UnsupportedVersion(version) => write!(
                formatter,
                "its version {version} is not the supported Machine IR version {}",
                scxml_compiler_core::MACHINE_IR_VERSION
            ),
            Self::Schema => formatter.write_str("it violates the closed Machine IR schema"),
            Self::Semantic => formatter.write_str("it breaks a Machine IR invariant"),
            Self::Unencodable => formatter.write_str("it has no canonical encoding"),
        }
    }
}

/// Closed stage of [`PackageError::Composition`]. Carries no payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositionStage {
    /// Encoding the data-only `ir-source` provider component.
    IrProvider,
    /// Encoding the data-only `script-bundle` provider component.
    ScriptBundle,
    /// Loading a component's type information into the composition graph.
    LoadPackage,
    /// Registering a loaded component with the composition graph.
    RegisterPackage,
    /// Aliasing an instance export (`ir-source`, `machine-api`, or
    /// `script-bundle`).
    AliasExport,
    /// Wiring the provider's `ir-source` into the machine instantiation.
    InstantiationArgument,
    /// Exporting `machine-api` or `script-bundle` from the composition.
    Export,
    /// Encoding and validating the composed component.
    Encode,
}

impl CompositionStage {
    /// Static, payload-free stage description.
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::IrProvider => "encoding the ir-source provider",
            Self::ScriptBundle => "encoding the script-bundle provider",
            Self::LoadPackage => "loading a component into the graph",
            Self::RegisterPackage => "registering a component with the graph",
            Self::AliasExport => "aliasing an instance export",
            Self::InstantiationArgument => "wiring the ir-source instantiation argument",
            Self::Export => "exporting an interface from the composition",
            Self::Encode => "encoding and validating the composed component",
        }
    }
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotLoweredIr => "packaging requires a lowered-ir artifact",
            Self::InvalidIr(cause) => {
                return write!(formatter, "Machine IR failed validation: {cause}");
            }
            Self::NonCanonicalIr => "Machine IR bytes are not canonical",
            Self::InconsistentRecipe => "package recipe does not describe the Machine IR",
            Self::TemplateSize => "machine template is empty or exceeds its byte bound",
            Self::InvalidTemplateDigest => "machine template digest is not lowercase SHA-256 hex",
            Self::TemplateDigestMismatch => "machine template bytes do not match the pinned digest",
            Self::InvalidTemplate => "machine template is not a valid component",
            Self::TemplateWorldMismatch => {
                "machine template does not implement the pinned machine world"
            }
            Self::Composition(stage) => {
                return write!(
                    formatter,
                    "component composition failed while {}",
                    stage.detail()
                );
            }
            Self::OutputTooLarge => "packaged component exceeds its byte bound",
            Self::MissingScript => "a pinned Wasm script component was not supplied",
            Self::UnexpectedScript => {
                "a supplied script is not a pinned Wasm script component or is duplicated"
            }
            Self::InvalidScript(cause) => {
                return write!(
                    formatter,
                    "a script component does not match its pin or the script world: {}",
                    cause.detail()
                );
            }
        })
    }
}

impl std::error::Error for PackageError {}

const fn category(error: PackageError) -> ErrorCategory {
    match error {
        PackageError::NotLoweredIr
        | PackageError::TemplateSize
        | PackageError::InvalidTemplateDigest
        | PackageError::MissingScript
        | PackageError::UnexpectedScript => ErrorCategory::InvalidInput,
        PackageError::InvalidIr(_)
        | PackageError::NonCanonicalIr
        | PackageError::InconsistentRecipe
        | PackageError::TemplateDigestMismatch
        | PackageError::InvalidTemplate
        | PackageError::TemplateWorldMismatch
        | PackageError::InvalidScript(_) => ErrorCategory::Validation,
        PackageError::Composition(_) | PackageError::OutputTooLarge => ErrorCategory::Packaging,
    }
}

/// Packages one `lowered-ir` artifact into a `component-wasm` artifact.
///
/// Preconditions are all checked, never assumed: `lowered` must be a
/// `lowered-ir` artifact whose bytes are canonical, valid Machine IR of the
/// current version and whose recipe digest matches them; `template` must pass
/// [`validate_template`]. On success the returned artifact holds validated
/// component bytes that import only the template's capability interfaces
/// (never `ir-source`) and export `machine-api`; its recipe records the
/// template digest and IR digest and has no next phase. Output is
/// byte-deterministic for identical inputs. Nothing is retained.
///
/// `scripts` must supply exactly the Wasm script components the IR pins,
/// one per URI, each authenticated against its pin
/// ([`scxml_script::verify_script`]); they are bundled and the package also
/// exports `script-bundle`. With no pinned Wasm scripts, `scripts` must be
/// empty and the package exports only `machine-api`.
///
/// # Errors
///
/// Returns a closed [`PackageError`]; no partial artifact is produced.
pub fn package(
    lowered: &PlannedArtifact,
    template: MachineTemplate<'_>,
    scripts: &[ScriptComponent<'_>],
) -> Result<PlannedArtifact, PackageError> {
    let span = observe(Module::Packager, Operation::PackagerPackage);
    let result = (|| {
        if lowered.kind != ArtifactKind::LoweredIr {
            return Err(PackageError::NotLoweredIr);
        }
        let machine = validate_ir(&lowered.bytes)?;
        let bundle = bundled_scripts(&machine, scripts)?;
        let ir_sha256 = hex::encode(Sha256::digest(&lowered.bytes));
        let recipe = &lowered.recipe;
        if recipe.ir_sha256 != ir_sha256
            || recipe.template_sha256.is_some()
            || recipe.next_phase != Some(ArtifactKind::ComponentWasm)
            || recipe.template_world != MACHINE_TEMPLATE_WORLD
            || recipe.template_version != MACHINE_TEMPLATE_VERSION
            || recipe.ir_source_interface != MACHINE_IR_SOURCE_INTERFACE
        {
            return Err(PackageError::InconsistentRecipe);
        }
        let bytes = compose(&lowered.bytes, template, &bundle)?;
        Ok(PlannedArtifact {
            kind: ArtifactKind::ComponentWasm,
            media_type: COMPONENT_WASM_MEDIA_TYPE.into(),
            file_extension: COMPONENT_WASM_FILE_EXTENSION.into(),
            bytes,
            recipe: PackageRecipe {
                template_world: MACHINE_TEMPLATE_WORLD.into(),
                template_version: MACHINE_TEMPLATE_VERSION.into(),
                template_sha256: Some(template.sha256.to_owned()),
                ir_sha256,
                ir_source_interface: MACHINE_IR_SOURCE_INTERFACE.into(),
                linked_extension_components: recipe.linked_extension_components.clone(),
                next_phase: None,
            },
        })
    })();
    match &result {
        Ok(_) => span.event(SignalEvent::ArtifactProduced),
        Err(error) => {
            span.event(SignalEvent::InputRejected);
            span.fail(category(*error));
        }
    }
    result
}

/// Decodes and validates untrusted IR bytes and requires canonical encoding.
fn validate_ir(ir: &[u8]) -> Result<MachineIr, PackageError> {
    let invalid = |error: LowerError| PackageError::InvalidIr(IrRejection::from_lower(&error));
    MachineIr::from_json(ir)
        .map_err(invalid)
        .and_then(|machine| {
            let canonical = machine.to_json().map_err(invalid)?;
            if canonical == ir {
                Ok(machine)
            } else {
                Err(PackageError::NonCanonicalIr)
            }
        })
}

/// Matches supplied scripts to the IR's pinned Wasm script components and
/// authenticates each one; returns them sorted by URI.
fn bundled_scripts<'a>(
    machine: &MachineIr,
    scripts: &[ScriptComponent<'a>],
) -> Result<Vec<ScriptComponent<'a>>, PackageError> {
    let pinned = machine
        .host_loaded_scripts()
        .into_iter()
        .filter_map(|source| {
            source
                .pinned
                .as_ref()
                .filter(|pin| pin.media_type == scxml_script::WASM_MEDIA_TYPE)
                .map(|pin| (source.uri.as_str(), pin.sha256.as_str()))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut supplied = std::collections::BTreeMap::new();
    for script in scripts {
        if !pinned.contains_key(script.uri) || supplied.insert(script.uri, *script).is_some() {
            return Err(PackageError::UnexpectedScript);
        }
    }
    let mut bundle = Vec::with_capacity(pinned.len());
    let mut total = 0_usize;
    for (uri, sha256) in pinned {
        let script = supplied.get(uri).ok_or(PackageError::MissingScript)?;
        scxml_script::verify_script(
            uri,
            scxml_script::WASM_MEDIA_TYPE,
            script.bytes,
            machine.data_model.as_deref(),
            Some(sha256),
        )
        .map_err(PackageError::InvalidScript)?;
        total = total
            .checked_add(script.bytes.len())
            .filter(|total| *total <= scxml_script::MAX_TOTAL_SCRIPT_BYTES)
            .ok_or(PackageError::InvalidScript(
                scxml_script::ScriptError::TooLarge,
            ))?;
        bundle.push(*script);
    }
    Ok(bundle)
}

/// Validates a caller-supplied generic machine template against its pin.
///
/// Checks, in order: byte bound, digest syntax, exact SHA-256 identity,
/// full component validation (default Wasm feature set), and the exact
/// import set [`TEMPLATE_IMPORTS`] and export set {[`MACHINE_API_INTERFACE`]}.
/// Pure and side-effect free.
///
/// # Errors
///
/// Returns the first failing check as a closed [`PackageError`].
pub fn validate_template(template: MachineTemplate<'_>) -> Result<(), PackageError> {
    let span = observe(Module::Packager, Operation::PackagerValidateTemplate);
    let result = (|| {
        if template.bytes.is_empty() || template.bytes.len() > MAX_TEMPLATE_BYTES {
            return Err(PackageError::TemplateSize);
        }
        if template.sha256.len() != 64
            || !template
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(PackageError::InvalidTemplateDigest);
        }
        if hex::encode(Sha256::digest(template.bytes)) != template.sha256 {
            return Err(PackageError::TemplateDigestMismatch);
        }
        if !wasmparser::Parser::is_component(template.bytes) {
            return Err(PackageError::InvalidTemplate);
        }
        wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::default())
            .validate_all(template.bytes)
            .map_err(|_| PackageError::InvalidTemplate)?;
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        // `parse_all` also yields the payloads of nested modules and
        // components; only the template's own top-level world (depth 1) is
        // its public contract. Depth is bounded by the validated bytes.
        let mut depth = 0_usize;
        for payload in wasmparser::Parser::new(0).parse_all(template.bytes) {
            match payload.map_err(|_| PackageError::InvalidTemplate)? {
                wasmparser::Payload::Version { .. } => depth = depth.saturating_add(1),
                wasmparser::Payload::End(_) => depth = depth.saturating_sub(1),
                wasmparser::Payload::ComponentImportSection(section) if depth == 1 => {
                    for import in section {
                        let import = import.map_err(|_| PackageError::InvalidTemplate)?;
                        imports.push(import.name.name.to_owned());
                    }
                }
                wasmparser::Payload::ComponentExportSection(section) if depth == 1 => {
                    for export in section {
                        let export = export.map_err(|_| PackageError::InvalidTemplate)?;
                        exports.push(export.name.name.to_owned());
                    }
                }
                _ => {}
            }
        }
        imports.sort();
        if imports != TEMPLATE_IMPORTS || exports != [MACHINE_API_INTERFACE] {
            return Err(PackageError::TemplateWorldMismatch);
        }
        Ok(())
    })();
    match &result {
        Ok(()) => span.event(SignalEvent::InputAccepted),
        Err(error) => {
            span.event(SignalEvent::InputRejected);
            span.fail(category(*error));
        }
    }
    result
}

/// Encodes the data-only `machine-ir-provider` component for `ir`.
///
/// The component contains one core module with a single non-growable memory
/// sized to the payload, one active data segment holding the canonical-ABI
/// return area followed by `ir` verbatim, and one function returning the
/// constant return-area pointer; the component lifts it as
/// `machine-ir: func() -> list<u8>` and exports it as
/// `stateforward:scxml/ir-source@0.2.0`. It imports nothing and has no
/// start function. Crate-private: [`package`] validates `ir` before calling
/// it; this function only bounds its size. Byte-deterministic for identical
/// input.
///
/// # Errors
///
/// Returns [`PackageError::InvalidIr`] ([`IrRejection::TooLarge`]) when `ir`
/// exceeds the Machine IR bound.
fn encode_ir_provider(ir: &[u8]) -> Result<Vec<u8>, PackageError> {
    use wasm_encoder::{
        CanonicalFunctionSection, CanonicalOption, CodeSection, Component, ComponentAliasSection,
        ComponentExportKind, ComponentExportSection, ComponentInstanceSection,
        ComponentTypeSection, ComponentValType, ConstExpr, DataSection, ExportKind, ExportSection,
        Function, FunctionSection, InstanceSection, MemorySection, MemoryType,
        Module as CoreModule, ModuleArg, ModuleSection, PrimitiveValType, TypeSection, ValType,
    };

    let too_large = PackageError::InvalidIr(IrRejection::TooLarge);
    if scxml_types::exceeds_wasm_bound(ir.len(), MAX_IR_BYTES) {
        return Err(too_large);
    }
    let length = u32::try_from(ir.len()).map_err(|_| too_large)?;
    let total = u64::from(IR_OFFSET) + u64::from(length);
    let pages = total.div_ceil(PAGE_BYTES).max(1);

    let mut image = Vec::with_capacity(ir.len().saturating_add(8));
    image.extend_from_slice(&IR_OFFSET.to_le_bytes());
    image.extend_from_slice(&length.to_le_bytes());
    image.extend_from_slice(ir);

    let mut module = CoreModule::new();
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I32]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: pages,
        maximum: Some(pages),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&memories);
    let mut exports = ExportSection::new();
    exports.export(PROVIDER_FUNCTION, ExportKind::Func, 0);
    exports.export(PROVIDER_MEMORY, ExportKind::Memory, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instructions().i32_const(0).end();
    code.function(&body);
    module.section(&code);
    let mut data = DataSection::new();
    data.active(0, &ConstExpr::i32_const(0), image);
    module.section(&data);

    let mut component = Component::new();
    component.section(&ModuleSection(&module));
    let mut core_instances = InstanceSection::new();
    core_instances.instantiate(0, Vec::<(&str, ModuleArg)>::new());
    component.section(&core_instances);
    let mut aliases = ComponentAliasSection::new();
    aliases.alias(wasm_encoder::Alias::CoreInstanceExport {
        instance: 0,
        kind: ExportKind::Func,
        name: PROVIDER_FUNCTION,
    });
    aliases.alias(wasm_encoder::Alias::CoreInstanceExport {
        instance: 0,
        kind: ExportKind::Memory,
        name: PROVIDER_MEMORY,
    });
    component.section(&aliases);
    let mut component_types = ComponentTypeSection::new();
    component_types
        .defined_type()
        .list(ComponentValType::Primitive(PrimitiveValType::U8));
    component_types
        .function()
        .params(Vec::<(&str, ComponentValType)>::new())
        .result(Some(ComponentValType::Type(0)));
    component.section(&component_types);
    let mut canonical = CanonicalFunctionSection::new();
    canonical.lift(0, 1, [CanonicalOption::Memory(0)]);
    component.section(&canonical);
    let mut instances = ComponentInstanceSection::new();
    instances.export_items([(PROVIDER_FUNCTION, ComponentExportKind::Func, 0)]);
    component.section(&instances);
    let mut component_exports = ComponentExportSection::new();
    component_exports.export(
        MACHINE_IR_SOURCE_INTERFACE,
        ComponentExportKind::Instance,
        0,
        None,
    );
    component.section(&component_exports);
    Ok(component.finish())
}

/// Internal bundle-encoding failure: [`package`] authenticates, bounds, and
/// sorts scripts by unique URI before encoding, so reaching this is a
/// violated internal precondition, reported as a composition stage.
const fn bundle_failure() -> PackageError {
    PackageError::Composition(CompositionStage::ScriptBundle)
}

/// Builds the bundle provider's memory image and its page count: the
/// return area, one pointer/length element per script, then the payloads.
fn script_bundle_image(scripts: &[ScriptComponent<'_>]) -> Result<(Vec<u8>, u64), PackageError> {
    let invalid = |_: std::num::TryFromIntError| bundle_failure();
    let count = u32::try_from(scripts.len()).map_err(invalid)?;
    if scripts.len() > scxml_script::MAX_SCRIPT_PACKAGES
        || scripts
            .windows(2)
            // `windows(2)` yields only slices of exactly two elements.
            .any(|pair| pair[0].uri >= pair[1].uri)
    {
        return Err(bundle_failure());
    }
    let table = 8_u32
        .checked_add(
            count
                .checked_mul(BUNDLE_ELEMENT_BYTES)
                .ok_or_else(bundle_failure)?,
        )
        .ok_or_else(bundle_failure)?;
    let mut elements = Vec::new();
    let mut payload = Vec::new();
    let mut cursor = table;
    let mut append = |bytes: &[u8], elements: &mut Vec<u8>| -> Result<(), PackageError> {
        let length = u32::try_from(bytes.len()).map_err(invalid)?;
        elements.extend_from_slice(&cursor.to_le_bytes());
        elements.extend_from_slice(&length.to_le_bytes());
        payload.extend_from_slice(bytes);
        cursor = cursor.checked_add(length).ok_or_else(bundle_failure)?;
        Ok(())
    };
    for script in scripts {
        scxml_script::validate_uri(script.uri).map_err(PackageError::InvalidScript)?;
        append(script.uri.as_bytes(), &mut elements)?;
        append(scxml_script::WASM_MEDIA_TYPE.as_bytes(), &mut elements)?;
        append(script.bytes, &mut elements)?;
    }
    let bound = scxml_script::MAX_TOTAL_SCRIPT_BYTES.saturating_add(
        scripts
            .len()
            .saturating_mul(scxml_script::MAX_SCRIPT_URI_BYTES + 64),
    );
    if payload.len() > bound {
        return Err(PackageError::InvalidScript(
            scxml_script::ScriptError::TooLarge,
        ));
    }
    let pages = u64::from(cursor).div_ceil(PAGE_BYTES).max(1);

    let mut image = Vec::with_capacity(payload.len().saturating_add(elements.len() + 8));
    image.extend_from_slice(&8_u32.to_le_bytes());
    image.extend_from_slice(&count.to_le_bytes());
    image.extend_from_slice(&elements);
    image.extend_from_slice(&payload);

    Ok((image, pages))
}

/// Encodes the data-only `script-bundle-provider` component for `scripts`.
///
/// The component contains one core module with a single non-growable
/// memory sized to the payload and one active data segment holding the
/// canonical-ABI return area, one 24-byte `(uri, media-type, bytes)`
/// pointer/length element per script, and the payload bytes verbatim; one
/// function returns the constant return-area pointer. The component lifts it
/// as `scripts: func() -> list<tuple<string, string, list<u8>>>` and exports
/// it as `stateforward:scxml/script-bundle@0.2.0`. It imports nothing and has
/// no start function. Crate-private: [`package`] passes scripts sorted by
/// unique URI and already authenticated; every entry's media type is
/// `application/wasm`. Byte-deterministic for identical input.
///
/// # Errors
///
/// [`PackageError::InvalidScript`] with [`scxml_script::ScriptError::TooLarge`]
/// when the payload exceeds [`scxml_script::MAX_TOTAL_SCRIPT_BYTES`] plus its
/// URI bytes, or with [`scxml_script::ScriptError::InvalidUri`] for an invalid
/// URI; [`PackageError::Composition`] with [`CompositionStage::ScriptBundle`]
/// when entries are unsorted, duplicated, or too many.
fn encode_script_bundle(scripts: &[ScriptComponent<'_>]) -> Result<Vec<u8>, PackageError> {
    use wasm_encoder::{
        CanonicalFunctionSection, CanonicalOption, CodeSection, Component, ComponentAliasSection,
        ComponentExportKind, ComponentExportSection, ComponentInstanceSection,
        ComponentTypeSection, ComponentValType, ConstExpr, DataSection, ExportKind, ExportSection,
        Function, FunctionSection, InstanceSection, MemorySection, MemoryType,
        Module as CoreModule, ModuleArg, ModuleSection, PrimitiveValType, TypeSection, ValType,
    };

    let (image, pages) = script_bundle_image(scripts)?;
    let mut module = CoreModule::new();
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I32]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    module.section(&functions);
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: pages,
        maximum: Some(pages),
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&memories);
    let mut exports = ExportSection::new();
    exports.export(BUNDLE_FUNCTION, ExportKind::Func, 0);
    exports.export(PROVIDER_MEMORY, ExportKind::Memory, 0);
    module.section(&exports);
    let mut code = CodeSection::new();
    let mut body = Function::new([]);
    body.instructions().i32_const(0).end();
    code.function(&body);
    module.section(&code);
    let mut data = DataSection::new();
    data.active(0, &ConstExpr::i32_const(0), image);
    module.section(&data);

    let mut component = Component::new();
    component.section(&ModuleSection(&module));
    let mut core_instances = InstanceSection::new();
    core_instances.instantiate(0, Vec::<(&str, ModuleArg)>::new());
    component.section(&core_instances);
    let mut aliases = ComponentAliasSection::new();
    aliases.alias(wasm_encoder::Alias::CoreInstanceExport {
        instance: 0,
        kind: ExportKind::Func,
        name: BUNDLE_FUNCTION,
    });
    aliases.alias(wasm_encoder::Alias::CoreInstanceExport {
        instance: 0,
        kind: ExportKind::Memory,
        name: PROVIDER_MEMORY,
    });
    component.section(&aliases);
    let mut component_types = ComponentTypeSection::new();
    component_types
        .defined_type()
        .list(ComponentValType::Primitive(PrimitiveValType::U8));
    component_types.defined_type().tuple([
        ComponentValType::Primitive(PrimitiveValType::String),
        ComponentValType::Primitive(PrimitiveValType::String),
        ComponentValType::Type(0),
    ]);
    component_types
        .defined_type()
        .list(ComponentValType::Type(1));
    component_types
        .function()
        .params(Vec::<(&str, ComponentValType)>::new())
        .result(Some(ComponentValType::Type(2)));
    component.section(&component_types);
    let mut canonical = CanonicalFunctionSection::new();
    canonical.lift(0, 3, [CanonicalOption::Memory(0), CanonicalOption::UTF8]);
    component.section(&canonical);
    let mut instances = ComponentInstanceSection::new();
    instances.export_items([(BUNDLE_FUNCTION, ComponentExportKind::Func, 0)]);
    component.section(&instances);
    let mut component_exports = ComponentExportSection::new();
    component_exports.export(
        scxml_script::SCRIPT_BUNDLE_INTERFACE,
        ComponentExportKind::Instance,
        0,
        None,
    );
    component.section(&component_exports);
    Ok(component.finish())
}

/// Maps any composition-library error to the closed [`CompositionStage`]
/// without retaining the library error (which may echo component names).
fn stage_failure<E>(stage: CompositionStage) -> impl FnOnce(E) -> PackageError {
    move |_| PackageError::Composition(stage)
}

/// Validates `template`, encodes the provider for `ir`, and composes them.
///
/// The composition instantiates the provider, passes its `ir-source` export
/// as the template's `ir-source` argument, re-imports the template's other
/// imports unchanged, and exports the template's `machine-api`. When
/// `scripts` is non-empty it also instantiates [`encode_script_bundle`]'s
/// provider and exports its `script-bundle`. The result is validated before
/// return. Crate-private: `ir` is Machine IR that [`package`] already
/// validated and `scripts` are already authenticated against it.
///
/// # Errors
///
/// Returns a closed [`PackageError`] from template validation, provider
/// encoding, the failing [`CompositionStage`], or the output bound.
fn compose(
    ir: &[u8],
    template: MachineTemplate<'_>,
    scripts: &[ScriptComponent<'_>],
) -> Result<Vec<u8>, PackageError> {
    validate_template(template)?;
    let provider = encode_ir_provider(ir)?;
    let mut graph = wac_graph::CompositionGraph::new();
    let machine = wac_graph::types::Package::from_bytes(
        "stateforward:machine",
        None,
        template.bytes.to_vec(),
        graph.types_mut(),
    )
    .map_err(stage_failure(CompositionStage::LoadPackage))?;
    let machine = graph
        .register_package(machine)
        .map_err(stage_failure(CompositionStage::RegisterPackage))?;
    let provider = wac_graph::types::Package::from_bytes(
        "stateforward:machine-ir",
        None,
        provider,
        graph.types_mut(),
    )
    .map_err(stage_failure(CompositionStage::LoadPackage))?;
    let provider = graph
        .register_package(provider)
        .map_err(stage_failure(CompositionStage::RegisterPackage))?;
    let provider_instance = graph.instantiate(provider);
    let machine_instance = graph.instantiate(machine);
    let ir_source = graph
        .alias_instance_export(provider_instance, MACHINE_IR_SOURCE_INTERFACE)
        .map_err(stage_failure(CompositionStage::AliasExport))?;
    graph
        .set_instantiation_argument(machine_instance, MACHINE_IR_SOURCE_INTERFACE, ir_source)
        .map_err(stage_failure(CompositionStage::InstantiationArgument))?;
    let machine_api = graph
        .alias_instance_export(machine_instance, MACHINE_API_INTERFACE)
        .map_err(stage_failure(CompositionStage::AliasExport))?;
    graph
        .export(machine_api, MACHINE_API_INTERFACE)
        .map_err(stage_failure(CompositionStage::Export))?;
    if !scripts.is_empty() {
        let bundle = wac_graph::types::Package::from_bytes(
            "stateforward:script-bundle",
            None,
            encode_script_bundle(scripts)?,
            graph.types_mut(),
        )
        .map_err(stage_failure(CompositionStage::LoadPackage))?;
        let bundle = graph
            .register_package(bundle)
            .map_err(stage_failure(CompositionStage::RegisterPackage))?;
        let bundle_instance = graph.instantiate(bundle);
        let bundle_export = graph
            .alias_instance_export(bundle_instance, scxml_script::SCRIPT_BUNDLE_INTERFACE)
            .map_err(stage_failure(CompositionStage::AliasExport))?;
        graph
            .export(bundle_export, scxml_script::SCRIPT_BUNDLE_INTERFACE)
            .map_err(stage_failure(CompositionStage::Export))?;
    }
    let bytes = graph
        .encode(wac_graph::EncodeOptions {
            define_components: true,
            validate: true,
            processor: None,
        })
        .map_err(stage_failure(CompositionStage::Encode))?;
    if scxml_types::exceeds_wasm_bound(bytes.len(), MAX_PACKAGED_COMPONENT_BYTES) {
        return Err(PackageError::OutputTooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Top-level (depth 1) import and export names of a component.
    fn component_world(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
        let mut imports = Vec::new();
        let mut exports = Vec::new();
        let mut depth = 0_usize;
        for payload in wasmparser::Parser::new(0).parse_all(bytes) {
            match payload.expect("fixture component parses") {
                wasmparser::Payload::Version { .. } => depth += 1,
                wasmparser::Payload::End(_) => depth -= 1,
                wasmparser::Payload::ComponentImportSection(section) if depth == 1 => {
                    for import in section {
                        imports.push(import.expect("import parses").name.name.to_owned());
                    }
                }
                wasmparser::Payload::ComponentExportSection(section) if depth == 1 => {
                    for export in section {
                        exports.push(export.expect("export parses").name.name.to_owned());
                    }
                }
                _ => {}
            }
        }
        (imports, exports)
    }

    fn validate(bytes: &[u8]) {
        wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::default())
            .validate_all(bytes)
            .expect("encoded provider validates as a component");
    }

    #[test]
    fn ir_provider_is_a_valid_import_free_data_only_ir_source_component() {
        let ir = br#"{"version":"fixture"}"#;
        let provider = encode_ir_provider(ir).expect("provider encodes");
        validate(&provider);
        assert_eq!(
            component_world(&provider),
            (Vec::new(), vec![MACHINE_IR_SOURCE_INTERFACE.to_owned()]),
            "the provider imports nothing and exports only ir-source"
        );
        assert!(
            provider.windows(ir.len()).any(|window| window == ir),
            "the IR bytes are embedded verbatim in the data segment"
        );
        assert_eq!(
            encode_ir_provider(ir).expect("provider encodes again"),
            provider,
            "provider encoding is byte-deterministic"
        );
    }

    /// The Machine IR byte bound applies to wasm builds only; natively only
    /// the provider's `u32` length field bounds the IR.
    #[cfg(target_arch = "wasm32")]
    #[test]
    fn wasm_only_ir_provider_rejects_ir_over_the_byte_bound() {
        assert_eq!(
            encode_ir_provider(&vec![b' '; MAX_IR_BYTES + 1]),
            Err(PackageError::InvalidIr(IrRejection::TooLarge))
        );
    }

    #[test]
    fn script_bundle_is_import_free_and_rejects_unsorted_entries() {
        let a = ScriptComponent {
            uri: "a.wasm",
            bytes: b"first",
        };
        let b = ScriptComponent {
            uri: "b.wasm",
            bytes: b"second",
        };
        let bundle = encode_script_bundle(&[a, b]).expect("sorted bundle encodes");
        validate(&bundle);
        assert_eq!(
            component_world(&bundle),
            (
                Vec::new(),
                vec![scxml_script::SCRIPT_BUNDLE_INTERFACE.to_owned()]
            ),
            "the bundle imports nothing and exports only script-bundle"
        );
        assert_eq!(
            encode_script_bundle(&[b, a]),
            Err(PackageError::Composition(CompositionStage::ScriptBundle)),
            "unsorted entries violate the internal precondition"
        );
        assert_eq!(
            encode_script_bundle(&[a, a]),
            Err(PackageError::Composition(CompositionStage::ScriptBundle)),
            "duplicate URIs violate the internal precondition"
        );
        let invalid_uri = ScriptComponent {
            uri: "",
            bytes: b"x",
        };
        assert_eq!(
            encode_script_bundle(&[invalid_uri]),
            Err(PackageError::InvalidScript(
                scxml_script::ScriptError::InvalidUri
            ))
        );
    }

    #[test]
    fn errors_preserve_closed_causes_without_payload() {
        let script = PackageError::InvalidScript(scxml_script::ScriptError::WorldMismatch);
        assert!(
            script
                .to_string()
                .ends_with(scxml_script::ScriptError::WorldMismatch.detail())
        );
        let composition = PackageError::Composition(CompositionStage::Encode);
        assert_eq!(
            composition.to_string(),
            "component composition failed while encoding and validating the composed component"
        );
        assert_eq!(category(script), ErrorCategory::Validation);
        assert_eq!(category(composition), ErrorCategory::Packaging);

        // Each Machine IR decode failure keeps a distinct closed cause.
        let causes = [
            (
                LowerError::LimitExceeded {
                    field: "IR bytes",
                    limit: MAX_IR_BYTES,
                },
                IrRejection::TooLarge,
            ),
            (
                LowerError::UnsupportedIrVersion(99),
                IrRejection::UnsupportedVersion(99),
            ),
            (LowerError::InvalidIrSchema, IrRejection::Schema),
            (
                LowerError::LimitExceeded {
                    field: "states",
                    limit: 1,
                },
                IrRejection::Semantic,
            ),
            (
                LowerError::InvalidReference("transition target"),
                IrRejection::Semantic,
            ),
            (
                LowerError::UnknownTarget("secret-state-name".into()),
                IrRejection::Semantic,
            ),
            (LowerError::IrSerialization, IrRejection::Unencodable),
        ];
        for (error, cause) in causes {
            assert_eq!(IrRejection::from_lower(&error), cause, "{error:?}");
            let rendered = PackageError::InvalidIr(cause).to_string();
            assert!(rendered.starts_with("Machine IR failed validation: "));
            assert!(
                !rendered.contains("secret-state-name"),
                "no input text is echoed"
            );
            assert_eq!(
                category(PackageError::InvalidIr(cause)),
                ErrorCategory::Validation
            );
        }
        assert!(
            PackageError::InvalidIr(IrRejection::UnsupportedVersion(99))
                .to_string()
                .contains("version 99")
        );
    }
}
