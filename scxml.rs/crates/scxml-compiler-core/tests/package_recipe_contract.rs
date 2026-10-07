//! Contract tests for canonical deterministic compiler package recipes.

use std::fmt::Write as _;

use scxml_compiler_core::{CompileInput, Compiler, MachineIr, NamespaceSchemaBinding};
use scxml_reporting::{
    ArtifactEnvelope, ArtifactError, CompilerOutput, CompilerStatus, decode_canonical,
    encode_canonical,
};
use scxml_syntax::Document;
use scxml_validator_core::{
    ExtensionPackageManifest, MemorySchemaResolver, Report, SUPPORTED_NAMESPACE_WIT_VERSION,
    SUPPORTED_NAMESPACE_WIT_WORLD, SchemaBackendError, SchemaMode, SchemaSource, SchemaValidator,
    Validator,
};
use sha2::{Digest, Sha256};

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

fn compile_fixture() -> scxml_compiler_core::CompileOutput {
    let compiler = Compiler::new(
        Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
            .with_schema_mode(SchemaMode::Off),
    );
    compiler.compile(CompileInput {
        source_name: "canonical-recipe.scxml".into(),
        source: r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="ready"><state id="ready"/></scxml>"#.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: Vec::new(),
        script_resolution: scxml_compiler_core::ScriptResolution::Compile,
        event_schemas: std::collections::BTreeMap::new(),
    })
}

fn package(
    namespace_uri: &str,
    extension_artifact: &str,
) -> (ExtensionPackageManifest, NamespaceSchemaBinding) {
    let schema_artifact = format!("schemas/{namespace_uri}.xsd");
    let schema_bytes = format!("schema for {namespace_uri}").into_bytes();
    (
        ExtensionPackageManifest {
            namespace_uri: namespace_uri.into(),
            package_version: "1.0.0".into(),
            extension_artifact: extension_artifact.into(),
            schema_artifact: schema_artifact.clone(),
            schema_sha256: hex::encode(Sha256::digest(&schema_bytes)),
            wit_world: SUPPORTED_NAMESPACE_WIT_WORLD.into(),
            wit_version: SUPPORTED_NAMESPACE_WIT_VERSION.into(),
            validator_artifact: None,
            validator_sha256: None,
        },
        NamespaceSchemaBinding {
            namespace_uri: namespace_uri.into(),
            schema: SchemaSource::new(schema_artifact, schema_bytes),
        },
    )
}

fn compile_custom(
    namespaces: &[(&str, &str)],
    packages: Vec<ExtensionPackageManifest>,
    package_schemas: Vec<NamespaceSchemaBinding>,
) -> scxml_compiler_core::CompileOutput {
    let mut declarations = String::new();
    let mut actions = String::new();
    for (index, (namespace_uri, local_name)) in namespaces.iter().enumerate() {
        write!(declarations, r#" xmlns:n{index}="{namespace_uri}""#)
            .expect("writing test namespace declarations to String must succeed");
        write!(actions, "<n{index}:{local_name}/>")
            .expect("writing test custom actions to String must succeed");
    }
    let compiler = Compiler::new(
        Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
            .with_schema_mode(SchemaMode::Off),
    );
    compiler.compile(CompileInput {
        source_name: "custom.scxml".into(),
        source: format!(
            r#"<scxml xmlns="http://www.w3.org/2005/07/scxml"{declarations} version="1.0" initial="ready"><state id="ready"><onentry>{actions}</onentry></state></scxml>"#
        ),
        packages,
        package_schemas,
        scripts: Vec::new(),
        script_resolution: scxml_compiler_core::ScriptResolution::Compile,
        event_schemas: std::collections::BTreeMap::new(),
    })
}

fn diagnostic_codes(output: &scxml_compiler_core::CompileOutput) -> Vec<&str> {
    output
        .validation
        .report
        .diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.code.as_str())
        .collect()
}

#[test]
fn accepted_compile_emits_deterministic_canonical_package_recipe() {
    let first = compile_fixture();
    let second = compile_fixture();

    let first_envelope = first
        .canonical_artifact()
        .expect("accepted compilation must produce a valid compiler envelope");
    let second_envelope = second
        .canonical_artifact()
        .expect("identical accepted compilation must produce a valid compiler envelope");
    let first_bytes = encode_canonical(&first_envelope)
        .expect("compiler envelope with canonical recipe must encode");
    let second_bytes = encode_canonical(&second_envelope)
        .expect("identical compiler envelope must encode deterministically");

    assert_eq!(first_bytes, second_bytes);
    assert_eq!(
        decode_canonical(&first_bytes).expect("canonical compiler bytes must decode"),
        first_envelope
    );

    let recipe = &first_envelope
        .compiler_payload()
        .expect("compiler envelope must expose compiler payload")
        .output
        .as_ref()
        .expect("accepted compilation must describe emitted output")
        .package_recipe;
    let value: serde_json::Value =
        serde_json::from_slice(recipe).expect("package recipe must be valid JSON");
    assert_eq!(
        serde_json::to_vec(&value).expect("package recipe value must serialize canonically"),
        *recipe
    );
}

#[test]
fn compiler_envelope_rejects_malformed_and_noncanonical_package_recipes() {
    for package_recipe in [b"{".to_vec(), b" {}".to_vec()] {
        assert_eq!(
            ArtifactEnvelope::compiler(
                "rejected-recipe.scxml",
                "<scxml/>",
                Vec::new(),
                CompilerStatus::Emitted,
                Some(CompilerOutput {
                    artifact_kind: "lowered-ir".into(),
                    media_type: "application/json".into(),
                    file_extension: "json".into(),
                    byte_length: 2,
                    sha256: "0".repeat(64),
                    package_recipe,
                }),
            ),
            Err(ArtifactError::NonCanonicalPackageRecipe)
        );
    }
}

#[test]
fn extension_namespace_without_package_fails_closed_even_when_schema_policy_is_off() {
    let output = compile_custom(&[("urn:test:custom", "action")], Vec::new(), Vec::new());
    let repeated = compile_custom(&[("urn:test:custom", "action")], Vec::new(), Vec::new());

    assert!(output.artifact.is_none());
    assert!(diagnostic_codes(&output).contains(&"COMP002"));
    let envelope = output
        .canonical_artifact()
        .expect("compiler package rejection must produce a canonical envelope");
    assert_eq!(
        envelope
            .compiler_payload()
            .expect("compiler envelope must contain compiler payload")
            .status,
        CompilerStatus::Rejected
    );
    assert_eq!(
        encode_canonical(&envelope).expect("package rejection must encode canonically"),
        encode_canonical(
            &repeated
                .canonical_artifact()
                .expect("repeated package rejection must remain canonical")
        )
        .expect("repeated package rejection must encode canonically")
    );
}

#[test]
fn an_unbound_namespace_is_reported_at_its_first_element_with_its_declaration() {
    let output = compile_custom(&[("urn:test:custom", "action")], Vec::new(), Vec::new());
    let source = &output.validation.source;
    let diagnostic = output
        .validation
        .report
        .diagnostics()
        .iter()
        .find(|diagnostic| diagnostic.code == "COMP002")
        .expect("the unbound namespace is COMP002");
    assert_eq!(
        diagnostic.message,
        "extension namespace `urn:test:custom` has no extension package"
    );
    assert_eq!(
        source.slice(diagnostic.primary.span),
        Some("n0:action"),
        "the primary label is the first element in the namespace"
    );
    let declared: Vec<_> = diagnostic
        .secondary
        .iter()
        .map(|label| source.slice(label.span))
        .collect();
    assert_eq!(declared, [Some("urn:test:custom")]);
    assert!(
        diagnostic
            .help
            .as_deref()
            .is_some_and(|help| help.contains("--extension-package")),
        "{:?}",
        diagnostic.help
    );
}

#[test]
fn built_in_xml_namespace_does_not_require_an_extension_package() {
    let compiler = Compiler::new(
        Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
            .with_schema_mode(SchemaMode::Off),
    );
    let output = compiler.compile(CompileInput {
        source_name: "xml-namespace.scxml".into(),
        source: r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" initial="ready"><state id="ready"><onentry><send event="out"><content><payload xml:lang="en">ok</payload></content></send></onentry></state></scxml>"#.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: Vec::new(),
        script_resolution: scxml_compiler_core::ScriptResolution::Compile,
        event_schemas: std::collections::BTreeMap::new(),
    });

    assert!(
        !diagnostic_codes(&output).contains(&"COMP002"),
        "built-in XML namespace must be exempt from extension package binding"
    );
    let artifact = output
        .artifact
        .expect("built-in XML namespace must not prevent artifact emission");
    assert!(artifact.recipe.linked_extension_components.is_empty());
    assert!(
        MachineIr::from_json(&artifact.bytes)
            .expect("emitted built-in namespace IR must decode")
            .extension_namespaces
            .is_empty()
    );
}

#[test]
fn package_recipe_links_exactly_used_extension_namespaces_in_canonical_order() {
    let (unused, unused_schema) = package("urn:test:unused", "components/unused.wasm");
    let (later, later_schema) = package("urn:z", "components/first-lexically.wasm");
    let (earlier, earlier_schema) = package("urn:a", "components/last-lexically.wasm");
    let output = compile_custom(
        &[("urn:z", "z-action"), ("urn:a", "a-action")],
        vec![unused, later, earlier],
        vec![unused_schema, later_schema, earlier_schema],
    );

    let recipe = &output
        .artifact
        .as_ref()
        .expect("all exact verified extension packages must compile")
        .recipe;
    assert_eq!(
        recipe.linked_extension_components,
        vec![
            "components/first-lexically.wasm",
            "components/last-lexically.wasm"
        ]
    );
    assert_eq!(
        MachineIr::from_json(
            &output
                .artifact
                .as_ref()
                .expect("verified compile must retain its artifact")
                .bytes
        )
        .expect("emitted IR must decode")
        .extension_namespaces,
        vec!["urn:a", "urn:z"]
    );
    output
        .canonical_artifact()
        .expect("verified namespace recipe must satisfy the canonical compiler contract");
}

#[test]
fn duplicate_or_mismatched_packages_and_invalid_unused_packages_fail_closed() {
    let (used, used_schema) = package("urn:test:used", "components/used.wasm");
    let duplicate = used.clone();
    let duplicate_output = compile_custom(
        &[("urn:test:used", "action")],
        vec![used, duplicate],
        vec![used_schema],
    );
    assert!(duplicate_output.artifact.is_none());
    assert!(diagnostic_codes(&duplicate_output).contains(&"COMP002"));

    let (other, other_schema) = package("urn:test:other", "components/other.wasm");
    let mismatch_output = compile_custom(
        &[("urn:test:used", "action")],
        vec![other],
        vec![other_schema],
    );
    assert!(mismatch_output.artifact.is_none());
    assert!(diagnostic_codes(&mismatch_output).contains(&"COMP002"));

    let (mut invalid_unused, invalid_unused_schema) =
        package("urn:test:unused", "components/unused.wasm");
    invalid_unused.schema_sha256 = "0".repeat(64);
    let invalid_unused_output =
        compile_custom(&[], vec![invalid_unused], vec![invalid_unused_schema]);
    assert!(invalid_unused_output.artifact.is_none());
    assert!(diagnostic_codes(&invalid_unused_output).contains(&"COMP002"));

    let (first, first_schema) = package("urn:test:first", "components/shared.wasm");
    let (second, second_schema) = package("urn:test:second", "components/shared.wasm");
    let duplicate_extension_component_output = compile_custom(
        &[
            ("urn:test:first", "first-action"),
            ("urn:test:second", "second-action"),
        ],
        vec![first, second],
        vec![first_schema, second_schema],
    );
    assert!(duplicate_extension_component_output.artifact.is_none());
    assert!(diagnostic_codes(&duplicate_extension_component_output).contains(&"COMP002"));

    let (used, used_schema) = package("urn:test:used", "components/used.wasm");
    let duplicate_schema_output = compile_custom(
        &[("urn:test:used", "action")],
        vec![used],
        vec![used_schema.clone(), used_schema],
    );
    assert!(duplicate_schema_output.artifact.is_none());
    assert!(diagnostic_codes(&duplicate_schema_output).contains(&"COMP002"));
}

#[test]
fn package_identity_fields_fail_closed_before_emission() {
    let (base, schema) = package("urn:test:used", "components/used.wasm");
    let mut invalid_packages = Vec::new();
    let mut empty_version = base.clone();
    empty_version.package_version.clear();
    invalid_packages.push(empty_version);
    let mut wrong_schema_artifact = base.clone();
    wrong_schema_artifact.schema_artifact = "schemas/wrong.xsd".into();
    invalid_packages.push(wrong_schema_artifact);
    let mut wrong_wit_world = base.clone();
    wrong_wit_world.wit_world = "stateforward:scxml/wrong".into();
    invalid_packages.push(wrong_wit_world);
    let mut wrong_wit_version = base;
    wrong_wit_version.wit_version = "9.9.9".into();
    invalid_packages.push(wrong_wit_version);

    for invalid_package in invalid_packages {
        let output = compile_custom(
            &[("urn:test:used", "action")],
            vec![invalid_package],
            vec![schema.clone()],
        );
        assert!(output.artifact.is_none());
        assert!(diagnostic_codes(&output).contains(&"COMP002"));
    }
}

#[test]
fn lowered_ir_recipe_names_the_ir_source_interface_and_the_component_phase() {
    let output = compile_fixture();
    let artifact = output
        .artifact
        .expect("valid flat source must produce a lowered IR artifact");
    assert_eq!(artifact.kind, scxml_compiler_core::ArtifactKind::LoweredIr);
    assert_eq!(
        artifact.recipe.ir_source_interface,
        scxml_compiler_core::MACHINE_IR_SOURCE_INTERFACE
    );
    assert_eq!(
        artifact.recipe.template_world,
        scxml_compiler_core::MACHINE_TEMPLATE_WORLD
    );
    assert_eq!(
        artifact.recipe.template_sha256, None,
        "lowered IR is not bound to any template yet"
    );
    assert_eq!(
        artifact.recipe.next_phase,
        Some(scxml_compiler_core::ArtifactKind::ComponentWasm)
    );
    let ir: MachineIr =
        serde_json::from_slice(&artifact.bytes).expect("lowered IR bytes must be JSON Machine IR");
    assert_eq!(ir.version, scxml_compiler_core::MACHINE_IR_VERSION);
}
