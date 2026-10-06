//! Compile-time `<script src>` resolution (SCXML 1.0 section 5.8): script
//! packages are authenticated and pinned into Machine IR; every failure is a
//! `COMP003` error on the exact `src` attribute value.

use scxml_compiler_core::{
    CompileInput, CompileOutput, Compiler, ExecutableContentIr, MachineIr, ScriptPackage,
    ScriptResolution,
};
use scxml_syntax::Document;
use scxml_validator_core::{
    MemorySchemaResolver, Report, SchemaBackendError, SchemaMode, SchemaSource, SchemaValidator,
    Validator,
};
use wasm_encoder::{
    Component, ComponentExportKind, ComponentExportSection, ComponentImportSection,
    ComponentInstanceSection, ComponentTypeRef, ComponentTypeSection, InstanceType,
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

const SOURCE: &str = r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><script>var input = 1;</script><state id="s"><onentry><script src="lib.wasm"/></onentry><transition event="go"><script src="lib.js"/></transition></state></scxml>"#;

/// A stand-in component with exactly the script world's names.
fn script_component(export: &str) -> Vec<u8> {
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

fn package(uri: &str, media_type: &str, bytes: Vec<u8>) -> ScriptPackage {
    ScriptPackage {
        uri: uri.into(),
        media_type: media_type.into(),
        sha256: scxml_script::sha256_hex(&bytes),
        bytes,
    }
}

fn compile(scripts: Vec<ScriptPackage>, resolution: ScriptResolution) -> CompileOutput {
    Compiler::new(
        Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
            .with_schema_mode(SchemaMode::Off),
    )
    .compile(CompileInput {
        source_name: "scripts.scxml".into(),
        source: SOURCE.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts,
        script_resolution: resolution,
        service_types: None,
        event_schemas: std::collections::BTreeMap::new(),
    })
}

fn valid_packages() -> Vec<ScriptPackage> {
    vec![
        package(
            "lib.wasm",
            scxml_script::WASM_MEDIA_TYPE,
            script_component(scxml_script::SCRIPT_API_INTERFACE),
        ),
        package("lib.js", "text/javascript", b"input = input + 1;".to_vec()),
    ]
}

/// `(code, spanned source text)` of every error diagnostic.
fn errors(output: &CompileOutput) -> Vec<(String, String)> {
    assert!(
        output.artifact.is_none(),
        "a rejected compile emits no artifact"
    );
    output
        .validation
        .report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.severity.as_str() == "error")
        .map(|diagnostic| {
            let span = diagnostic.primary.span;
            (
                diagnostic.code.clone(),
                SOURCE
                    .get(span.start..span.end)
                    .unwrap_or("<outside>")
                    .to_owned(),
            )
        })
        .collect()
}

fn script_sources(machine: &MachineIr) -> Vec<(String, Option<String>, String)> {
    let mut found = Vec::new();
    let mut blocks: Vec<&[ExecutableContentIr]> = Vec::new();
    for state in &machine.states {
        for block in &state.onentry {
            blocks.push(&block.actions);
        }
    }
    for transition in &machine.transitions {
        blocks.push(&transition.actions);
    }
    for action in blocks.into_iter().flatten() {
        if let ExecutableContentIr::Script {
            src: Some(src),
            body,
        } = action
        {
            found.push((
                src.uri.clone(),
                src.pinned.as_ref().map(|pinned| pinned.media_type.clone()),
                body.clone(),
            ));
        }
    }
    found
}

#[test]
fn compile_time_resolution_pins_components_and_embeds_data_model_text() {
    let packages = valid_packages();
    let wasm_digest = packages[0].sha256.clone();
    let output = compile(packages, ScriptResolution::Compile);
    let artifact = output.artifact.expect("valid script packages compile");
    let machine = MachineIr::from_json(&artifact.bytes).expect("pinned IR is valid");
    assert_eq!(
        script_sources(&machine),
        [
            (
                "lib.wasm".to_owned(),
                Some(scxml_script::WASM_MEDIA_TYPE.to_owned()),
                String::new()
            ),
            (
                "lib.js".to_owned(),
                Some("text/javascript".to_owned()),
                "input = input + 1;".to_owned()
            ),
        ]
    );
    assert_eq!(
        artifact.recipe.linked_extension_components,
        [format!("script-sha256:{wasm_digest}")],
        "the recipe pins exactly the Wasm script component"
    );
    let hosted = machine.host_loaded_scripts();
    assert_eq!(hosted.len(), 1, "only the component needs the host at load");
    assert_eq!(hosted[0].uri, "lib.wasm");
}

#[test]
fn runtime_resolution_leaves_references_unpinned() {
    let output = compile(Vec::new(), ScriptResolution::Runtime);
    let artifact = output
        .artifact
        .expect("runtime resolution compiles without packages");
    let machine = MachineIr::from_json(&artifact.bytes).expect("IR is valid");
    assert!(
        script_sources(&machine)
            .iter()
            .all(|(_, pinned, body)| pinned.is_none() && body.is_empty())
    );
    assert_eq!(machine.host_loaded_scripts().len(), 2);
    assert_eq!(
        errors(&compile(valid_packages(), ScriptResolution::Runtime))[0].0,
        "COMP003",
        "runtime resolution takes no packages"
    );
}

#[test]
fn unresolvable_or_invalid_packages_are_comp003_on_the_src_value() {
    let src = |uri: &str| ("COMP003".to_owned(), uri.to_owned());
    assert_eq!(
        errors(&compile(Vec::new(), ScriptResolution::Compile)),
        [src("lib.wasm"), src("lib.js")],
        "missing packages point at each exact src value"
    );

    let mut mismatched = valid_packages();
    mismatched[0].sha256 = "0".repeat(64);
    assert_eq!(
        errors(&compile(mismatched, ScriptResolution::Compile)),
        [src("lib.wasm")]
    );

    let mut not_component = valid_packages();
    not_component[0] = package(
        "lib.wasm",
        scxml_script::WASM_MEDIA_TYPE,
        b"\0asm\x01\0\0\0".to_vec(),
    );
    assert_eq!(
        errors(&compile(not_component, ScriptResolution::Compile)),
        [src("lib.wasm")]
    );

    let mut wrong_world = valid_packages();
    wrong_world[0] = package(
        "lib.wasm",
        scxml_script::WASM_MEDIA_TYPE,
        script_component("stateforward:scxml/machine-api@0.2.0"),
    );
    assert_eq!(
        errors(&compile(wrong_world, ScriptResolution::Compile)),
        [src("lib.wasm")]
    );

    let mut unknown_media = valid_packages();
    unknown_media[1].media_type = "application/x-shellscript".into();
    assert_eq!(
        errors(&compile(unknown_media, ScriptResolution::Compile)),
        [src("lib.js")]
    );

    // Script text has no size bound outside a wasm build.
    let mut large = valid_packages();
    large[1] = package("lib.js", "text/javascript", vec![b' '; 65 * 1024]);
    assert!(
        compile(large, ScriptResolution::Compile).artifact.is_some(),
        "65 KiB of script text compiles"
    );

    let mut unused = valid_packages();
    unused.push(package("other.wasm", "text/javascript", b"x".to_vec()));
    let unused = errors(&compile(unused, ScriptResolution::Compile));
    assert_eq!(unused.len(), 1);
    assert_eq!(unused[0].0, "COMP003", "unused packages fail closed");

    let mut duplicated = valid_packages();
    duplicated.push(duplicated[1].clone());
    assert_eq!(
        errors(&compile(duplicated, ScriptResolution::Compile))[0].0,
        "COMP003"
    );
}

#[test]
fn src_with_inline_content_is_rejected_before_lowering() {
    let output = Compiler::new(
        Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
            .with_schema_mode(SchemaMode::Off),
    )
    .compile(CompileInput {
        source_name: "both.scxml".into(),
        source: r#"<scxml xmlns="http://www.w3.org/2005/07/scxml" version="1.0" datamodel="ecmascript" initial="s"><state id="s"><onentry><script src="lib.js">x = 1;</script></onentry></state></scxml>"#.into(),
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: Vec::new(),
        script_resolution: ScriptResolution::Runtime,
        service_types: None,
        event_schemas: std::collections::BTreeMap::new(),
    });
    assert!(output.artifact.is_none());
    assert!(
        output
            .validation
            .report
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == "SCXML050"),
        "src XOR content is a validator error"
    );
}

#[test]
fn one_uri_cannot_carry_two_pins() {
    let source = SOURCE.replace("lib.js", "lib.wasm");
    let bytes = script_component(scxml_script::SCRIPT_API_INTERFACE);
    let output = Compiler::new(
        Validator::new(MemorySchemaResolver::new(), NoSchemaBackend)
            .with_schema_mode(SchemaMode::Off),
    )
    .compile(CompileInput {
        source_name: "twice.scxml".into(),
        source,
        packages: Vec::new(),
        package_schemas: Vec::new(),
        scripts: vec![package("lib.wasm", scxml_script::WASM_MEDIA_TYPE, bytes)],
        script_resolution: ScriptResolution::Compile,
        service_types: None,
        event_schemas: std::collections::BTreeMap::new(),
    });
    let ir = output
        .artifact
        .expect("one package serves both references")
        .bytes;
    let text = String::from_utf8(ir).expect("IR is UTF-8");
    let pinned = MachineIr::from_json(text.as_bytes()).expect("consistent pins decode");
    assert_eq!(pinned.host_loaded_scripts().len(), 1, "one load per URI");
    // Re-pin only the first reference to another digest.
    let marker = "\"sha256\": \"";
    let at = text.find(marker).expect("IR carries a pin") + marker.len();
    let mut forged = text.clone();
    forged.replace_range(at..at + 64, &"0".repeat(64));
    assert!(
        MachineIr::from_json(forged.as_bytes()).is_err(),
        "conflicting pins for one URI are invalid IR"
    );
}
