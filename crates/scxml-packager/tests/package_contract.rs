//! Hermetic packaging contract tests.
//!
//! The real generic machine template is exercised end to end (validated,
//! composed, instantiated under Wasmtime, and compared with the native
//! engine) by `scripts/verify-wasm-capabilities.sh` and the Wasmtime host
//! tests. These tests pin the packager's own contract against a minimal
//! stand-in template that has exactly the machine world's import and export
//! names and the exact `ir-source` instance type.

use scxml_compiler_core::{
    ArtifactKind, CompileInput, Compiler, MACHINE_IR_SOURCE_INTERFACE, PlannedArtifact,
};
use scxml_packager::{
    IrRejection, MACHINE_API_INTERFACE, MAX_TEMPLATE_BYTES, MachineTemplate, PackageError,
    SCRIPTED_PACKAGE_EXPORTS, ScriptComponent, TEMPLATE_IMPORTS, package, validate_template,
};
use scxml_syntax::Document;
use scxml_validator_core::{
    MemorySchemaResolver, Report, SchemaBackendError, SchemaMode, SchemaSource, SchemaValidator,
    Validator,
};
use sha2::{Digest as _, Sha256};
use wasm_encoder::{
    Component, ComponentExportKind, ComponentExportSection, ComponentImportSection,
    ComponentInstanceSection, ComponentTypeRef, ComponentTypeSection, ComponentValType,
    InstanceType, PrimitiveValType,
};

#[derive(Debug)]
struct NoSchemaBackend;

impl SchemaValidator for NoSchemaBackend {
    fn validate(
        &self,
        _document: &Document,
        _schemas: &[SchemaSource],
    ) -> Result<Report, SchemaBackendError> {
        Ok(Report::new())
    }
}

fn lowered(source: &str) -> PlannedArtifact {
    Compiler::new(
        Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
            .with_schema_mode(SchemaMode::Off),
    )
    .compile(CompileInput {
        source_name: "packaged.scxml".into(),
        source: source.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: Vec::new(),
        script_resolution: scxml_compiler_core::ScriptResolution::Compile,
        service_types: None,
        event_schemas: std::collections::BTreeMap::new(),
    })
    .artifact
    .expect("fixture SCXML lowers")
}

const HIERARCHY: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="work">
  <state id="work" initial="a">
    <history id="h" type="deep"><transition target="a"/></history>
    <state id="a"><transition event="next" target="b"/></state>
    <state id="b"><transition event="leave" target="pause"/></state>
  </state>
  <parallel id="pause">
    <state id="left"/><state id="right"/>
    <transition event="resume" target="h"/>
  </parallel>
</scxml>"#;

/// Builds a stand-in template: imports the four machine-world instances by
/// their exact names (with the exact `ir-source` instance type) and exports
/// an empty `machine-api` instance.
fn stand_in_template(imports: &[&str], export: &str) -> Vec<u8> {
    let mut component = Component::new();
    let mut types = ComponentTypeSection::new();
    let mut ir_source = InstanceType::new();
    ir_source
        .ty()
        .defined_type()
        .list(ComponentValType::Primitive(PrimitiveValType::U8));
    ir_source
        .ty()
        .function()
        .params(Vec::<(&str, ComponentValType)>::new())
        .result(Some(ComponentValType::Type(0)));
    ir_source.export("machine-ir", ComponentTypeRef::Func(1));
    types.instance(&ir_source);
    // Every other import gets its own distinct instance type (one uniquely
    // named nullary function), as the real template's interfaces do.
    for index in 0..imports.len() {
        let mut capability = InstanceType::new();
        capability
            .ty()
            .function()
            .params(Vec::<(&str, ComponentValType)>::new())
            .result(None);
        capability.export(
            format!("capability{index}").as_str(),
            ComponentTypeRef::Func(0),
        );
        types.instance(&capability);
    }
    component.section(&types);
    let mut import_section = ComponentImportSection::new();
    for (index, name) in imports.iter().enumerate() {
        let ty = if *name == MACHINE_IR_SOURCE_INTERFACE {
            0
        } else {
            u32::try_from(index + 1).expect("fixture type index fits u32")
        };
        import_section.import(*name, ComponentTypeRef::Instance(ty));
    }
    component.section(&import_section);
    let mut instances = ComponentInstanceSection::new();
    instances.export_items(Vec::<(&str, ComponentExportKind, u32)>::new());
    component.section(&instances);
    let mut exports = ComponentExportSection::new();
    exports.export(
        export,
        ComponentExportKind::Instance,
        u32::try_from(imports.len()).expect("fixture import count fits u32"),
        None,
    );
    component.section(&exports);
    component.finish()
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn component_world(bytes: &[u8]) -> (Vec<String>, Vec<String>) {
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut depth = 0_usize;
    for payload in wasmparser::Parser::new(0).parse_all(bytes) {
        match payload.expect("packaged component parses") {
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
    imports.sort();
    (imports, exports)
}

#[test]
fn packaging_composes_a_terminal_component_that_no_longer_imports_ir_source() {
    let lowered = lowered(HIERARCHY);
    let template_bytes = stand_in_template(&TEMPLATE_IMPORTS, MACHINE_API_INTERFACE);
    let template_sha256 = digest(&template_bytes);
    let template = MachineTemplate {
        bytes: &template_bytes,
        sha256: &template_sha256,
    };
    let packaged = package(&lowered, template, &[]).expect("stand-in template packages");
    let again = package(&lowered, template, &[]).expect("stand-in template packages again");
    assert_eq!(packaged, again, "packaging is byte-deterministic");

    assert_eq!(packaged.kind, ArtifactKind::ComponentWasm);
    assert_eq!(packaged.media_type, "application/wasm");
    assert_eq!(
        packaged.recipe.next_phase, None,
        "component-wasm is terminal"
    );
    assert_eq!(
        packaged.recipe.template_sha256.as_deref(),
        Some(template_sha256.as_str())
    );
    assert_eq!(packaged.recipe.ir_sha256, lowered.recipe.ir_sha256);
    assert_eq!(
        packaged.recipe.ir_source_interface,
        MACHINE_IR_SOURCE_INTERFACE
    );
    wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::default())
        .validate_all(&packaged.bytes)
        .expect("packaged component validates");
    let (imports, exports) = component_world(&packaged.bytes);
    let expected_imports = TEMPLATE_IMPORTS
        .iter()
        .filter(|name| **name != MACHINE_IR_SOURCE_INTERFACE)
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        imports, expected_imports,
        "ir-source is satisfied internally"
    );
    assert_eq!(exports, vec![MACHINE_API_INTERFACE.to_owned()]);
    assert!(
        packaged
            .bytes
            .windows(lowered.bytes.len())
            .any(|window| window == lowered.bytes),
        "the packaged component embeds the exact lowered IR bytes"
    );

    let other = self::lowered(
        r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="x"><state id="x"/></scxml>"#,
    );
    let other = package(&other, template, &[]).expect("another machine packages");
    assert_ne!(
        other.bytes, packaged.bytes,
        "distinct IR yields distinct bytes"
    );
}

#[test]
fn packaging_rejects_untrusted_ir_and_inconsistent_recipes() {
    let good = lowered(HIERARCHY);
    let template_bytes = stand_in_template(&TEMPLATE_IMPORTS, MACHINE_API_INTERFACE);
    let template_sha256 = digest(&template_bytes);
    let template = MachineTemplate {
        bytes: &template_bytes,
        sha256: &template_sha256,
    };

    let mut not_ir = good.clone();
    not_ir.kind = ArtifactKind::ComponentWasm;
    assert_eq!(
        package(&not_ir, template, &[]),
        Err(PackageError::NotLoweredIr)
    );

    let mut malformed = good.clone();
    malformed.bytes = b"{\"version\": 3".to_vec();
    assert_eq!(
        package(&malformed, template, &[]),
        Err(PackageError::InvalidIr(IrRejection::Schema))
    );

    let mut future: serde_json::Value =
        serde_json::from_slice(&good.bytes).expect("lowered IR is JSON");
    future["version"] = serde_json::json!(scxml_compiler_core::MACHINE_IR_VERSION + 1);
    let mut wrong_version = good.clone();
    wrong_version.bytes = serde_json::to_vec_pretty(&future).expect("future IR encodes");
    // IR from another release names its version, distinct from malformed IR.
    let rejected = package(&wrong_version, template, &[]);
    assert_eq!(
        rejected,
        Err(PackageError::InvalidIr(IrRejection::UnsupportedVersion(
            scxml_compiler_core::MACHINE_IR_VERSION + 1
        )))
    );
    assert!(
        rejected
            .expect_err("a future version is rejected")
            .to_string()
            .contains("version"),
        "the rendered error names the version cause"
    );

    let mut dangling: serde_json::Value =
        serde_json::from_slice(&good.bytes).expect("lowered IR is JSON");
    dangling["initial"]["targets"] = serde_json::json!([u32::MAX]);
    let mut semantic = good.clone();
    semantic.bytes = serde_json::to_vec_pretty(&dangling).expect("dangling IR encodes");
    assert_eq!(
        package(&semantic, template, &[]),
        Err(PackageError::InvalidIr(IrRejection::Semantic))
    );

    let mut compact = good.clone();
    let value: serde_json::Value = serde_json::from_slice(&good.bytes).expect("lowered IR is JSON");
    compact.bytes = serde_json::to_vec(&value).expect("compact IR encodes");
    assert_eq!(
        package(&compact, template, &[]),
        Err(PackageError::NonCanonicalIr),
        "only canonical IR bytes are packaged"
    );

    let mut stale_recipe = good.clone();
    stale_recipe.recipe.ir_sha256 = "0".repeat(64);
    assert_eq!(
        package(&stale_recipe, template, &[]),
        Err(PackageError::InconsistentRecipe)
    );
    let mut already_bound = good;
    already_bound.recipe.template_sha256 = Some(template_sha256.clone());
    assert_eq!(
        package(&already_bound, template, &[]),
        Err(PackageError::InconsistentRecipe)
    );
}

/// The Machine IR byte bound applies to wasm builds only.
#[cfg(target_arch = "wasm32")]
#[test]
fn wasm_only_packaging_rejects_ir_over_the_byte_bound() {
    let mut oversized = lowered(HIERARCHY);
    oversized.bytes = vec![b' '; scxml_compiler_core::MAX_IR_BYTES + 1];
    let template_bytes = stand_in_template(&TEMPLATE_IMPORTS, MACHINE_API_INTERFACE);
    let template_sha256 = digest(&template_bytes);
    let template = MachineTemplate {
        bytes: &template_bytes,
        sha256: &template_sha256,
    };
    assert_eq!(
        package(&oversized, template, &[]),
        Err(PackageError::InvalidIr(IrRejection::TooLarge))
    );
}

#[test]
fn template_validation_fails_closed_before_composition() {
    let good = stand_in_template(&TEMPLATE_IMPORTS, MACHINE_API_INTERFACE);
    let good_sha256 = digest(&good);
    let check = |bytes: &[u8], sha256: &str| validate_template(MachineTemplate { bytes, sha256 });

    assert_eq!(check(&good, &good_sha256), Ok(()));
    assert_eq!(check(&[], &digest(&[])), Err(PackageError::TemplateSize));
    let oversized = vec![0_u8; MAX_TEMPLATE_BYTES + 1];
    assert_eq!(
        check(&oversized, &digest(&oversized)),
        Err(PackageError::TemplateSize)
    );
    assert_eq!(
        check(&good, &good_sha256.to_uppercase()),
        Err(PackageError::InvalidTemplateDigest)
    );
    assert_eq!(
        check(&good, "abc"),
        Err(PackageError::InvalidTemplateDigest)
    );
    assert_eq!(
        check(&good, &"0".repeat(64)),
        Err(PackageError::TemplateDigestMismatch)
    );
    let garbage = b"\0asm-but-not-a-component".to_vec();
    assert_eq!(
        check(&garbage, &digest(&garbage)),
        Err(PackageError::InvalidTemplate)
    );
    let mut truncated = good.clone();
    truncated.truncate(good.len() - 3);
    assert_eq!(
        check(&truncated, &digest(&truncated)),
        Err(PackageError::InvalidTemplate)
    );

    // A valid component with the wrong world: an import-free component that
    // exports only ir-source, shaped like the data-only IR provider.
    let provider = stand_in_template(&[], MACHINE_IR_SOURCE_INTERFACE);
    assert_eq!(
        check(&provider, &digest(&provider)),
        Err(PackageError::TemplateWorldMismatch)
    );
    // Missing, extra, or renamed imports and exports are world mismatches.
    let missing = stand_in_template(&TEMPLATE_IMPORTS[1..], MACHINE_API_INTERFACE);
    assert_eq!(
        check(&missing, &digest(&missing)),
        Err(PackageError::TemplateWorldMismatch)
    );
    let mut extra_names = TEMPLATE_IMPORTS.to_vec();
    extra_names.push("stateforward:scxml/data-model-host@0.2.0");
    let extra = stand_in_template(&extra_names, MACHINE_API_INTERFACE);
    assert_eq!(
        check(&extra, &digest(&extra)),
        Err(PackageError::TemplateWorldMismatch)
    );
    let renamed = stand_in_template(&TEMPLATE_IMPORTS, "stateforward:scxml/machine-api@0.3.0");
    assert_eq!(
        check(&renamed, &digest(&renamed)),
        Err(PackageError::TemplateWorldMismatch)
    );
}

/// A stand-in Wasm script component with exactly the script world's names.
fn stand_in_script(export: &str) -> Vec<u8> {
    let mut component = Component::new();
    let mut types = ComponentTypeSection::new();
    types.instance(&InstanceType::new());
    component.section(&types);
    let mut imports = ComponentImportSection::new();
    for name in scxml_script::SCRIPT_ALLOWED_IMPORTS {
        imports.import(name, ComponentTypeRef::Instance(0));
    }
    component.section(&imports);
    let mut instances = ComponentInstanceSection::new();
    instances.export_items(Vec::<(&str, ComponentExportKind, u32)>::new());
    component.section(&instances);
    let mut exports = ComponentExportSection::new();
    exports.export(export, ComponentExportKind::Instance, 3, None);
    component.section(&exports);
    component.finish()
}

const SCRIPTED: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><onentry><script src="b.wasm"/><script src="a.wasm"/></onentry></state></scxml>"#;

fn lowered_with_scripts(scripts: &[(&str, &[u8])]) -> PlannedArtifact {
    Compiler::new(
        Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
            .with_schema_mode(SchemaMode::Off),
    )
    .compile(CompileInput {
        source_name: "scripted.scxml".into(),
        source: SCRIPTED.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: scripts
            .iter()
            .map(|(uri, bytes)| scxml_compiler_core::ScriptPackage {
                uri: (*uri).into(),
                media_type: scxml_script::WASM_MEDIA_TYPE.into(),
                sha256: digest(bytes),
                bytes: bytes.to_vec(),
            })
            .collect(),
        script_resolution: scxml_compiler_core::ScriptResolution::Compile,
        service_types: None,
        event_schemas: std::collections::BTreeMap::new(),
    })
    .artifact
    .expect("scripted fixture lowers with pinned scripts")
}

#[test]
fn pinned_script_components_are_bundled_and_exported() {
    let a = stand_in_script(scxml_script::SCRIPT_API_INTERFACE);
    let mut b = stand_in_script(scxml_script::SCRIPT_API_INTERFACE);
    // Distinct bytes for the second script: append a custom section.
    b.extend_from_slice(&[0, 2, 1, b'x']);
    let lowered = lowered_with_scripts(&[("a.wasm", &a), ("b.wasm", &b)]);
    let template_bytes = stand_in_template(&TEMPLATE_IMPORTS, MACHINE_API_INTERFACE);
    let template_sha256 = digest(&template_bytes);
    let template = MachineTemplate {
        bytes: &template_bytes,
        sha256: &template_sha256,
    };
    let scripts = [
        ScriptComponent {
            uri: "b.wasm",
            bytes: &b,
        },
        ScriptComponent {
            uri: "a.wasm",
            bytes: &a,
        },
    ];
    let packaged = package(&lowered, template, &scripts).expect("pinned scripts bundle");
    let again = package(&lowered, template, &scripts).expect("bundling repeats");
    assert_eq!(packaged, again, "scripted packaging is byte-deterministic");
    wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::default())
        .validate_all(&packaged.bytes)
        .expect("scripted package validates");
    let (_, exports) = component_world(&packaged.bytes);
    assert_eq!(exports, SCRIPTED_PACKAGE_EXPORTS.map(str::to_owned));
    for script in [&a, &b] {
        assert!(
            packaged
                .bytes
                .windows(script.len())
                .any(|window| window == script.as_slice()),
            "the package embeds each exact script component"
        );
    }
    assert_eq!(
        package(&lowered, template, &scripts[..1]),
        Err(PackageError::MissingScript)
    );
    assert_eq!(
        package(&lowered, template, &[]),
        Err(PackageError::MissingScript)
    );
    let extra = [
        scripts[0],
        scripts[1],
        ScriptComponent {
            uri: "c.wasm",
            bytes: &a,
        },
    ];
    assert_eq!(
        package(&lowered, template, &extra),
        Err(PackageError::UnexpectedScript)
    );
    let swapped = [
        ScriptComponent {
            uri: "a.wasm",
            bytes: &b,
        },
        ScriptComponent {
            uri: "b.wasm",
            bytes: &a,
        },
    ];
    assert_eq!(
        package(&lowered, template, &swapped),
        Err(PackageError::InvalidScript(
            scxml_script::ScriptError::DigestMismatch
        )),
        "each script must match its own pin, and the cause is preserved"
    );
    let plain = lowered_free_of_scripts();
    assert_eq!(
        package(&plain, template, &scripts),
        Err(PackageError::UnexpectedScript),
        "scripts are bundled only when the IR pins them"
    );
}

fn lowered_free_of_scripts() -> PlannedArtifact {
    lowered(HIERARCHY)
}
